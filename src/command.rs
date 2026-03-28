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
            .with_context(|| format!("Binding command listener to {}", addr))?;
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
