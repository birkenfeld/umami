// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::net;
use std::time::{Duration, Instant};
use anyhow::Context;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use uds::UnixDatagramExt;
use crate::{ldebug, lprintln};
use crate::channel::{RecvTimeoutError, Sender};
use crate::error::UResult;
use crate::params::ParamMap;
use crate::pipeline::PipeItem;

pub type ModuleId = internment::Intern<String>;

/// Cap on how long a single command waits for replies from inputs/postprocessor.
/// A wedged component is a data-integrity problem that needs a human to look at
/// it regardless; this timeout's only job is to keep the command socket itself
/// responsive (so unrelated commands like `Ping` aren't collateral damage) and
/// to report the failure clearly instead of hanging forever.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Builds the standard "gave up waiting" reply, also logging it locally since
/// an unresponsive `who` is worth noticing even if nobody's watching the reply.
fn unresponsive(who: &str) -> CommandReply {
    lprintln!(ERROR, "{who} did not respond within {REPLY_TIMEOUT:?}");
    CommandReply::new_error(format!(
        "{who} did not respond within {REPLY_TIMEOUT:?}; it may be stuck"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Clear,
    Start { run_id: String },
    Stop,
    Reset,
    GetState,
    SetRawDump { enable: bool, path: String },
    GetModes,
    SetMode { name: String },
    GetParams { #[serde(default)] full: bool },
    SetParams { params: ParamMap },
    SaveHisto { path: String, max_nt: usize },
    SaveConfig { path: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandReply {
    Ok,
    Data { value: Value },
    Error { module: Option<ModuleId>, message: String },
}

impl CommandReply {
    pub fn new_error(message: String) -> Self {
        CommandReply::Error { module: None, message }
    }

    pub fn new_mod_error(module: ModuleId, message: String) -> Self {
        CommandReply::Error { module: Some(module), message }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, CommandReply::Error { .. })
    }
}


pub struct CommandHandler {
    sock: net::UnixDatagram,
    input_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
    post_send: Sender<PipeItem>,
    config_path: std::path::PathBuf,
}

impl CommandHandler {
    pub fn new(
        socket_name: &str,
        input_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
        post_send: Sender<PipeItem>,
        config_path: std::path::PathBuf,
    ) -> UResult<Self> {
        let addr = uds::UnixSocketAddr::from_abstract(socket_name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = net::UnixDatagram::bind_unix_addr(&addr)
            .with_context(|| format!("Binding command listener to {addr}"))?;
        Ok(Self { sock, input_send, post_send, config_path })
    }

    pub fn start(mut self) -> anyhow::Result<()> {
        std::thread::Builder::new()
            .name("Command handler".into())
            .spawn(move || self.main())
            .context("Spawning command handler thread")?;
        Ok(())
    }

    pub fn main(&mut self) {
        let mut buf = [0u8; 8192];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, addr)) => {
                    let reply = self.message(&buf[..n]);
                    let serialized = serde_json::to_string(&reply).expect("serializable");
                    if let Err(e) = self.sock.send_to_addr(serialized.as_bytes(), &addr) {
                        lprintln!(ERROR, "Failed to send command reply to {addr:?}: {e:#}");
                    }
                }
                Err(e) => {
                    lprintln!(ERROR, "Unix socket receive error: {e:#}");
                }
            }
        }
    }

    fn message(&mut self, buf: &[u8]) -> CommandReply {
        match str::from_utf8(buf) {
            Err(_) => {
                CommandReply::new_error("Invalid UTF-8 in telegram".into())
            }
            Ok(s) => match serde_json::from_str::<Command>(s) {
                Err(e) => {
                    lprintln!(ERROR, "Received invalid command {s:?}: {e:#}");
                    CommandReply::new_error(format!("Invalid JSON or invalid command: {e:#}"))
                }
                Ok(cmd) => {
                    ldebug!("Received command: {cmd:?}");
                    let reply = self.handle(cmd);
                    ldebug!("Command reply: {reply:?}");
                    reply
                }
            },
        }
    }

    /// Sends `item` to the postprocessor and waits for one reply on `rep_recv`,
    /// both bounded by `deadline`; on timeout, returns [`unresponsive`] instead
    /// of blocking forever.
    fn post_and_wait(
        &self, item: PipeItem, rep_recv: &crate::channel::Receiver<CommandReply>, deadline: Instant,
    ) -> CommandReply {
        if self.post_send.send_deadline(item, deadline).is_err() {
            return unresponsive("Postprocessor");
        }
        rep_recv.recv_deadline(deadline).unwrap_or_else(|_| unresponsive("Postprocessor"))
    }

    pub fn handle(&self, cmd: Command) -> CommandReply {
        let (rep_send, rep_recv) = crate::channel::bounded(self.input_send.len());
        let deadline = Instant::now() + REPLY_TIMEOUT;
        match cmd {
            Command::Ping => {
                let version = format!("UMAMI {}", env!("CARGO_PKG_VERSION"));
                CommandReply::Data { value: version.into() }
            }
            Command::Clear => {
                match self.post_send.send_deadline(PipeItem::Clear, deadline) {
                    Ok(()) => CommandReply::Ok,
                    Err(_) => unresponsive("Postprocessor"),
                }
            }
            Command::GetState => self.post_and_wait(PipeItem::GetState(rep_send), &rep_recv, deadline),
            Command::GetModes => self.post_and_wait(PipeItem::GetModes(rep_send), &rep_recv, deadline),
            Command::SetMode { name } => self.post_and_wait(
                PipeItem::SetMode(ModuleId::new(name), rep_send), &rep_recv, deadline),
            Command::SaveHisto { path, max_nt } => self.post_and_wait(
                PipeItem::SaveHisto(path, max_nt, rep_send), &rep_recv, deadline),
            Command::Start { .. } | Command::Stop | Command::SetRawDump { .. } |
            Command::Reset => {
                if let Command::Start { run_id } = &cmd
                    && self.post_send.send_deadline(PipeItem::StartOfRun(run_id.into()), deadline).is_err()
                {
                    return unresponsive("Postprocessor");
                }
                let cmd_and_send = (cmd, rep_send);
                for (name, sender) in &self.input_send {
                    if sender.send_deadline(cmd_and_send.clone(), deadline).is_err() {
                        return unresponsive(&format!("Input {name}"));
                    }
                }
                let mut replies = Vec::with_capacity(self.input_send.len());
                for _ in 0..self.input_send.len() {
                    match rep_recv.recv_deadline(deadline) {
                        Ok(reply) => replies.push(reply),
                        Err(_) => return unresponsive("An input"),
                    }
                }
                replies.into_iter().find(|r| r.is_error()).unwrap_or(CommandReply::Ok)
            }
            Command::GetParams { full } => {
                let mut map = ParamMap::new();

                // gather from each input's recipe first, using the shared fixed-count
                // channel (same pattern as the Start/Stop/Reset fan-out above)
                for (name, sender) in &self.input_send {
                    if sender.send_deadline(
                        (Command::GetParams { full }, rep_send.clone()), deadline,
                    ).is_err() {
                        return unresponsive(&format!("Input {name}"));
                    }
                }
                for _ in 0..self.input_send.len() {
                    match rep_recv.recv_deadline(deadline) {
                        Ok(CommandReply::Data { value: Value::Object(obj) }) => map.extend(obj),
                        Ok(reply) if reply.is_error() => return reply,
                        Ok(_) => {} // shouldn't happen
                        Err(_) => return unresponsive("An input"),
                    }
                }

                // this command needs a differently typed channel
                let (rep_send, rep_recv) = crate::channel::unbounded();
                if self.post_send.send_deadline(PipeItem::GetParams(full, rep_send), deadline).is_err() {
                    return unresponsive("Postprocessor");
                }
                // aggregate parameters from all HasParams into a single map
                loop {
                    match rep_recv.recv_deadline(deadline) {
                        Ok((name, params)) => for (param, info) in params {
                            map.insert(format!("{name}.{param}"), info);
                        },
                        Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => return unresponsive("A recipe or output"),
                    }
                }
                CommandReply::Data { value: map.into() }
            }
            Command::SetParams { params } => {
                // validate keys up front, before touching any channel
                for name in params.keys() {
                    if !name.contains('.') {
                        return CommandReply::new_error(
                            format!("Invalid param key {name}, needs to be of the \
                                     form <module>.<param>")
                        );
                    }
                }

                // broadcast the full (unsplit) map to every input; each applies only
                // the portion addressed to its own recipe name and no-ops otherwise
                for (name, sender) in &self.input_send {
                    if sender.send_deadline(
                        (Command::SetParams { params: params.clone() }, rep_send.clone()), deadline,
                    ).is_err() {
                        return unresponsive(&format!("Input {name}"));
                    }
                }
                for _ in 0..self.input_send.len() {
                    match rep_recv.recv_deadline(deadline) {
                        Ok(reply) if reply.is_error() => return reply,
                        Ok(_) => {}
                        Err(_) => return unresponsive("An input"),
                    }
                }

                // parse parameters from single map into multiple maps, for the
                // postprocessor's recipes and the outputs
                let mut new_map = BTreeMap::new();
                for (name, value) in params {
                    let (module, param) = name.split_once('.').expect("checked above");
                    new_map.entry(ModuleId::new(module.into()))
                           .or_insert_with(ParamMap::new)
                           .insert(param.into(), value);
                }

                // new pair to not mix input and postprocessor replies on the same channel
                let (rep_send, rep_recv) = crate::channel::unbounded();
                if self.post_send.send_deadline(PipeItem::SetParams(new_map, rep_send), deadline).is_err() {
                    return unresponsive("Postprocessor");
                }
                // aggregate errors, if any
                loop {
                    match rep_recv.recv_deadline(deadline) {
                        Ok(reply) if reply.is_error() => return reply,
                        Ok(_) => {}
                        Err(RecvTimeoutError::Disconnected) => return CommandReply::Ok,
                        Err(RecvTimeoutError::Timeout) => return unresponsive("A recipe or output"),
                    }
                }
            }
            Command::SaveConfig { path } => self.save_config(path),
        }
    }

    /// Gathers every settable, non-runtime-only param's current value and
    /// patches it into the original config file (`self.config_path`),
    /// writing the result to `path`, or back to `self.config_path` if
    /// `path` is `None`.
    fn save_config(&self, path: Option<String>) -> CommandReply {
        let params = match self.handle(Command::GetParams { full: true }) {
            CommandReply::Data { value: Value::Object(map) } => map,
            reply if reply.is_error() => return reply,
            _ => return CommandReply::new_error("Unexpected reply gathering params".into()),
        };

        let mut updates = HashMap::new();
        for (key, info) in &params {
            let Some((module, param)) = key.split_once('.') else { continue };
            if param == "_info" {
                continue;
            }
            let Some(info) = info.as_object() else { continue };
            if info.get("readonly").and_then(Value::as_bool).unwrap_or(false)
                || info.get("runtime_only").and_then(Value::as_bool).unwrap_or(false)
            {
                continue;
            }
            let Some(value) = info.get("value") else { continue };
            if value.is_null() {
                continue;
            }
            updates.insert((module, param), value);
        }

        let target = path.map_or_else(|| self.config_path.clone(), Into::into);
        match crate::config::patch_config_file(&self.config_path, &target, &updates) {
            Ok(()) => CommandReply::Ok,
            Err(e) => CommandReply::new_error(format!("Failed to save config: {e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use crate::channel::Receiver;
    use crate::params::ParamMap;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    type InputRecv = Receiver<(Command, Sender<CommandReply>)>;

    /// Builds a CommandHandler wired to fake input-module and postprocessor
    /// channels the test can drive/inspect directly, without any real inputs,
    /// postprocessor, or outputs running.
    fn make_handler(module_names: &[&str])
        -> (CommandHandler, BTreeMap<ModuleId, InputRecv>, Receiver<PipeItem>)
    {
        make_handler_at(module_names, "test.conf".into())
    }

    /// Like [`make_handler`], but with a config file path the test controls
    /// (needed for `SaveConfig`, which reads/writes it for real).
    fn make_handler_at(module_names: &[&str], config_path: std::path::PathBuf)
        -> (CommandHandler, BTreeMap<ModuleId, InputRecv>, Receiver<PipeItem>)
    {
        let mut input_send = BTreeMap::new();
        let mut input_recvs = BTreeMap::new();
        for name in module_names {
            let (s, r) = crate::channel::unbounded();
            let id = ModuleId::new((*name).to_string());
            input_send.insert(id, s);
            input_recvs.insert(id, r);
        }
        let (post_send, post_recv) = crate::channel::unbounded();
        let sock_name = format!("umami_cmdtest_{}_{}",
                                COUNTER.fetch_add(1, Ordering::SeqCst), std::process::id());
        let handler = CommandHandler::new(&sock_name, input_send, post_send, config_path).unwrap();
        (handler, input_recvs, post_recv)
    }

    /// Replies to exactly one request on an input-module channel with `reply`.
    fn respond_to_input(recv: InputRecv, reply: CommandReply) {
        std::thread::spawn(move || {
            let (_cmd, rep) = recv.recv().unwrap();
            rep.send(reply).unwrap();
        });
    }

    /// Replies to exactly one postprocessor meta-request with `reply`.
    fn respond_to_post_meta(post_recv: Receiver<PipeItem>, reply: CommandReply) {
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::GetModes(s) | PipeItem::SetMode(_, s)
                | PipeItem::GetState(s) | PipeItem::SaveHisto(_, _, s) => {
                    s.send(reply).unwrap();
                }
                other => panic!("unexpected item sent to postprocessor: {other:?}"),
            }
        });
    }

    #[test]
    fn test_ping_and_clear() {
        let (handler, _inputs, post_recv) = make_handler(&[]);

        match handler.handle(Command::Ping) {
            CommandReply::Data { value } =>
                assert!(value.as_str().unwrap().starts_with("UMAMI ")),
            other => panic!("unexpected reply: {other:?}"),
        }

        // Clear is fire-and-forget: no reply is awaited, just forwarded
        assert!(matches!(handler.handle(Command::Clear), CommandReply::Ok));
        assert!(matches!(post_recv.recv_timeout(Duration::from_secs(5)).unwrap(), PipeItem::Clear));
    }

    #[test]
    fn test_get_modes_passes_through_postprocessor_reply() {
        let (handler, _inputs, post_recv) = make_handler(&[]);
        respond_to_post_meta(post_recv, CommandReply::Data { value: vec!["std", "tof"].into() });
        match handler.handle(Command::GetModes) {
            CommandReply::Data { value } => {
                let modes: Vec<_> = value.as_array().unwrap().iter()
                    .map(|v| v.as_str().unwrap()).collect();
                assert_eq!(modes, vec!["std", "tof"]);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn test_set_mode_get_state_save_histo_pass_through_errors() {
        // each of these forwards one request to the postprocessor and returns
        // exactly what it replies with -- verify that including the error case
        for cmd in [
            Command::SetMode { name: "bogus".into() },
            Command::GetState,
            Command::SaveHisto { path: "/tmp/x".into(), max_nt: 1 },
        ] {
            let (handler, _inputs, post_recv) = make_handler(&[]);
            respond_to_post_meta(post_recv, CommandReply::new_error("boom".into()));
            assert!(handler.handle(cmd).is_error());
        }
    }

    #[test]
    fn test_start_sends_start_of_run_and_fans_out_to_inputs() {
        let (handler, inputs, post_recv) = make_handler(&["mod0"]);
        for recv in inputs.into_values() {
            respond_to_input(recv, CommandReply::Ok);
        }
        let reply = handler.handle(Command::Start { run_id: "run1".into() });
        assert!(matches!(reply, CommandReply::Ok));
        match post_recv.recv_timeout(Duration::from_secs(5)).unwrap() {
            PipeItem::StartOfRun(run_id) => assert_eq!(run_id, "run1"),
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_fanout_returns_first_input_error() {
        let (handler, inputs, _post_recv) = make_handler(&["mod0", "mod1"]);
        let mut inputs = inputs.into_iter();
        let (_, recv0) = inputs.next().unwrap();
        let (_, recv1) = inputs.next().unwrap();
        respond_to_input(recv0, CommandReply::Ok);
        respond_to_input(recv1, CommandReply::new_error("input failed".into()));

        assert!(handler.handle(Command::Stop).is_error());
    }

    #[test]
    fn test_set_params_rejects_key_without_dot() {
        let (handler, _inputs, _post_recv) = make_handler(&[]);
        let mut params = ParamMap::new();
        params.insert("no_dot_here".into(), serde_json::json!(1));
        // never touches any channel: validated up front
        assert!(handler.handle(Command::SetParams { params }).is_error());
    }

    #[test]
    fn test_set_params_forwards_split_module_and_param() {
        let (handler, _inputs, post_recv) = make_handler(&[]);
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::SetParams(map, send) => {
                    assert_eq!(map.len(), 1);
                    let (name, p) = map.into_iter().next().unwrap();
                    assert_eq!(name, ModuleId::new("std".into()));
                    assert_eq!(p["bin_x"], 4);
                    send.send(CommandReply::Ok).unwrap();
                }
                other => panic!("unexpected item: {other:?}"),
            }
        });
        let mut params = ParamMap::new();
        params.insert("std.bin_x".into(), serde_json::json!(4));
        assert!(matches!(handler.handle(Command::SetParams { params }), CommandReply::Ok));
    }

    #[test]
    fn test_get_params_aggregates_into_dotted_keys() {
        let (handler, _inputs, post_recv) = make_handler(&[]);
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::GetParams(_full, send) => {
                    let mut p1 = ParamMap::new();
                    p1.insert("bin_x".into(), serde_json::json!(1));
                    send.send((ModuleId::new("std".into()), p1)).unwrap();
                    let mut p2 = ParamMap::new();
                    p2.insert("threshold".into(), serde_json::json!(5));
                    send.send((ModuleId::new("mesy".into()), p2)).unwrap();
                    // dropping `send` here closes the channel, ending the aggregation loop
                }
                other => panic!("unexpected item: {other:?}"),
            }
        });
        match handler.handle(Command::GetParams { full: false }) {
            CommandReply::Data { value } => {
                assert_eq!(value["std.bin_x"], 1);
                assert_eq!(value["mesy.threshold"], 5);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn test_get_params_merges_inputs_and_postprocessor() {
        let (handler, inputs, post_recv) = make_handler(&["ge01"]);
        let recv0 = inputs.into_values().next().unwrap();
        std::thread::spawn(move || {
            let (cmd, rep) = recv0.recv().unwrap();
            assert!(matches!(cmd, Command::GetParams { full: false }));
            let mut value = serde_json::Map::new();
            value.insert("ge.rebin_8x8".into(), serde_json::json!({"value": false}));
            rep.send(CommandReply::Data { value: value.into() }).unwrap();
        });
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::GetParams(_full, send) => {
                    let mut p = ParamMap::new();
                    p.insert("bin_x".into(), serde_json::json!(1));
                    send.send((ModuleId::new("std".into()), p)).unwrap();
                    // dropping `send` here closes the channel, ending the aggregation loop
                }
                other => panic!("unexpected item: {other:?}"),
            }
        });
        match handler.handle(Command::GetParams { full: false }) {
            CommandReply::Data { value } => {
                assert_eq!(value["ge.rebin_8x8"]["value"], false);
                assert_eq!(value["std.bin_x"], 1);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    /// SaveConfig patches settable values into the original file, skips
    /// readonly/runtime_only ones, and leaves unrelated content (comments,
    /// other sections) untouched.
    #[test]
    fn test_save_config_patches_settable_values_and_skips_readonly_and_runtime_only() {
        let path = std::env::temp_dir().join(format!(
            "umami_saveconfig_test_{}_{}.conf", std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)));
        std::fs::write(&path, r#"
# a comment that should survive
[inputs.ge01]
type = "ge"
source = "localhost:50000"
channel_offset = 0

[process_modes]
default = "std"
std = { type = "histo_std", bin_x = 1 }
"#).unwrap();

        let (handler, inputs, post_recv) = make_handler_at(&["ge01"], path.clone());
        let recv0 = inputs.into_values().next().unwrap();
        std::thread::spawn(move || {
            let (cmd, rep) = recv0.recv().unwrap();
            assert!(matches!(cmd, Command::GetParams { full: true }));
            let mut value = serde_json::Map::new();
            value.insert("ge01.channel_offset".into(), serde_json::json!({
                "value": 5, "datatype": "u32", "help": "", "readonly": false, "runtime_only": false,
            }));
            value.insert("ge01.mod_types".into(), serde_json::json!({
                "value": ["mpsd8"], "datatype": "", "help": "", "readonly": true, "runtime_only": false,
            }));
            rep.send(CommandReply::Data { value: value.into() }).unwrap();
        });
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::GetParams(full, send) => {
                    assert!(full);
                    let mut p = ParamMap::new();
                    p.insert("bin_x".into(), serde_json::json!({
                        "value": 4, "datatype": "u16", "help": "", "readonly": false, "runtime_only": false,
                    }));
                    p.insert("pulser".into(), serde_json::json!({
                        "value": {"0": {"on": true}}, "datatype": "", "help": "",
                        "readonly": false, "runtime_only": true,
                    }));
                    send.send((ModuleId::new("std".into()), p)).unwrap();
                }
                other => panic!("unexpected item: {other:?}"),
            }
        });

        assert!(matches!(
            handler.handle(Command::SaveConfig { path: None }), CommandReply::Ok));

        let saved = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(saved.contains("a comment that should survive"));
        assert!(saved.contains("source = \"localhost:50000\""), "saved:\n{saved}");
        let doc: toml_edit::DocumentMut = saved.parse().unwrap();
        assert_eq!(doc["inputs"]["ge01"]["channel_offset"].as_integer(), Some(5));
        assert_eq!(doc["process_modes"]["std"]["bin_x"].as_integer(), Some(4));
        // readonly/runtime_only fields must not have been written
        assert!(doc["inputs"]["ge01"].get("mod_types").is_none());
        assert!(doc["process_modes"]["std"].get("pulser").is_none());
    }

    #[test]
    fn test_get_params_propagates_input_error() {
        let (handler, inputs, _post_recv) = make_handler(&["ge01"]);
        let recv0 = inputs.into_values().next().unwrap();
        respond_to_input(recv0, CommandReply::new_error("recipe get_params failed".into()));
        assert!(handler.handle(Command::GetParams { full: false }).is_error());
    }

    /// SetParams forwards the whole (unsplit) dotted map to every input, so
    /// each can pick out only the keys addressed to its own recipe name --
    /// the split-by-module map is only computed for the postprocessor/outputs.
    #[test]
    fn test_set_params_broadcasts_full_map_to_inputs() {
        let (handler, inputs, post_recv) = make_handler(&["ge01"]);
        let recv0 = inputs.into_values().next().unwrap();
        std::thread::spawn(move || {
            let (cmd, rep) = recv0.recv().unwrap();
            match cmd {
                Command::SetParams { params } => {
                    assert_eq!(params.len(), 2);
                    assert_eq!(params["ge.rebin_8x8"], true);
                    assert_eq!(params["std.bin_x"], 4);
                }
                other => panic!("unexpected command: {other:?}"),
            }
            rep.send(CommandReply::Ok).unwrap();
        });
        std::thread::spawn(move || {
            match post_recv.recv().unwrap() {
                PipeItem::SetParams(map, send) => {
                    assert_eq!(map.len(), 1);
                    send.send(CommandReply::Ok).unwrap();
                }
                other => panic!("unexpected item: {other:?}"),
            }
        });
        let mut params = ParamMap::new();
        params.insert("ge.rebin_8x8".into(), serde_json::json!(true));
        params.insert("std.bin_x".into(), serde_json::json!(4));
        assert!(matches!(handler.handle(Command::SetParams { params }), CommandReply::Ok));
    }

    #[test]
    fn test_set_params_propagates_input_error_before_reaching_postprocessor() {
        let (handler, inputs, _post_recv) = make_handler(&["ge01"]);
        let recv0 = inputs.into_values().next().unwrap();
        respond_to_input(recv0, CommandReply::new_error("bad value".into()));
        let mut params = ParamMap::new();
        params.insert("ge.rebin_8x8".into(), serde_json::json!("not a bool"));
        assert!(handler.handle(Command::SetParams { params }).is_error());
    }

    /// A wedged postprocessor or input must not hang `handle()` forever -- it
    /// should give up around REPLY_TIMEOUT and report a clear error instead.
    #[test]
    fn test_unresponsive_component_times_out_instead_of_hanging() {
        // nothing ever reads `post_recv`, simulating a wedged postprocessor
        let (handler, _inputs, _post_recv) = make_handler(&[]);
        let started = Instant::now();
        assert!(handler.handle(Command::GetState).is_error());
        assert!(started.elapsed() >= REPLY_TIMEOUT, "returned before the deadline");
        assert!(started.elapsed() < REPLY_TIMEOUT * 2, "took far longer than the deadline");

        // same for a wedged input during the Start/Stop/Reset/SetRawDump fan-out
        let (handler, _inputs, _post_recv) = make_handler(&["mod0"]);
        let started = Instant::now();
        assert!(handler.handle(Command::Stop).is_error());
        assert!(started.elapsed() >= REPLY_TIMEOUT, "returned before the deadline");
        assert!(started.elapsed() < REPLY_TIMEOUT * 2, "took far longer than the deadline");
    }
}
