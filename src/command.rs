// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::os::unix::net;
use anyhow::Context;
use itertools::Itertools;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use uds::UnixDatagramExt;
use crate::{ldebug, lprintln};
use crate::channel::Sender;
use crate::error::UResult;
use crate::params::ParamMap;
use crate::pipeline::PipeItem;

pub type ModuleId = internment::Intern<String>;

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
    GetParams,
    SetParams { params: ParamMap },
    SaveHisto { path: String, max_nt: usize },
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
}

impl CommandHandler {
    pub fn new(
        socket_name: &str,
        input_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
        post_send: Sender<PipeItem>,
    ) -> UResult<Self> {
        let addr = uds::UnixSocketAddr::from_abstract(socket_name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = net::UnixDatagram::bind_unix_addr(&addr)
            .with_context(|| format!("Binding command listener to {addr}"))?;
        Ok(Self { sock, input_send, post_send })
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

    pub fn handle(&self, cmd: Command) -> CommandReply {
        let (rep_send, rep_recv) = crate::channel::bounded(self.input_send.len());
        match cmd {
            Command::Ping => {
                let version = format!("UMAMI {}", env!("CARGO_PKG_VERSION"));
                CommandReply::Data { value: version.into() }
            }
            Command::Clear => {
                self.post_send.send(PipeItem::Clear)
                              .expect("postprocessor command receiver died");
                CommandReply::Ok
            }
            Command::GetState => {
                self.post_send.send(PipeItem::GetState(rep_send))
                              .expect("postprocessor command receiver died");
                rep_recv.recv().expect("postprocessor command sender died")
            }
            Command::GetModes => {
                self.post_send.send(PipeItem::GetModes(rep_send))
                              .expect("postprocessor command receiver died");
                rep_recv.recv().expect("postprocessor command sender died")
            }
            Command::SetMode { name } => {
                self.post_send.send(PipeItem::SetMode(ModuleId::new(name), rep_send))
                              .expect("postprocessor command receiver died");
                rep_recv.recv().expect("postprocessor command sender died")
            }
            Command::Start { .. } | Command::Stop | Command::SetRawDump { .. } |
            Command::Reset => {
                if let Command::Start { run_id } = &cmd {
                    self.post_send.send(PipeItem::StartOfRun(run_id.into()))
                                  .expect("postprocessor command receiver died");
                }
                let cmd_and_send = (cmd, rep_send);
                for sender in self.input_send.values() {
                    sender.send(cmd_and_send.clone()).expect("input command receiver died");
                }
                let replies = (0..self.input_send.len()).map(
                    |_| rep_recv.recv().expect("input command sender died")
                ).collect_vec();
                if let Some(err) = replies.into_iter().find(|r| r.is_error()) {
                    return err;
                }
                CommandReply::Ok
            }
            Command::SaveHisto { path, max_nt } => {
                self.post_send.send(PipeItem::SaveHisto(path, max_nt, rep_send))
                              .expect("postprocessor command receiver died");
                rep_recv.recv().expect("postprocessor command sender died")
            }
            Command::GetParams => {
                // this command need a differently typed channel
                let (rep_send, rep_recv) = crate::channel::unbounded();
                self.post_send.send(PipeItem::GetParams(rep_send))
                              .expect("postprocessor command receiver died");
                // aggregate parameters from all HasParams into a single map
                let mut map = ParamMap::new();
                for (name, params) in rep_recv {
                    for (param, info) in params {
                        map.insert(format!("{name}.{param}"), info);
                    }
                }
                CommandReply::Data { value: map.into() }
            }
            Command::SetParams { params } => {
                // parse parameters from single map into multiple maps
                let mut new_map = BTreeMap::new();
                for (name, value) in params {
                    if !name.contains('.') {
                        return CommandReply::new_error(
                            format!("Invalid param key {name}, needs to be of the \
                                     form <module>.<param>")
                        );
                    }
                    let (module, param) = name.split_once('.').expect("checked");
                    new_map.entry(ModuleId::new(module.into()))
                           .or_insert_with(ParamMap::new)
                           .insert(param.into(), value);
                }

                self.post_send.send(PipeItem::SetParams(new_map, rep_send))
                              .expect("postprocessor command receiver died");
                // aggregate errors, if any
                let errors = rep_recv.into_iter().filter(|r| r.is_error()).collect_vec();
                if errors.is_empty() {
                    CommandReply::Ok
                } else {
                    // TODO aggregate error messages into one reply
                    errors.into_iter().next().unwrap()
                }
            }
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
        let handler = CommandHandler::new(&sock_name, input_send, post_send).unwrap();
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
                PipeItem::GetParams(send) => {
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
        match handler.handle(Command::GetParams) {
            CommandReply::Data { value } => {
                assert_eq!(value["std.bin_x"], 1);
                assert_eq!(value["mesy.threshold"], 5);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }
}
