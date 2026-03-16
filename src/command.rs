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
use crate::event::ModuleId;
use crate::pipeline::PipeItem;

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
    SetMode { name: String, params: toml::Table },
    Config { module: ModuleId, name: String, value: Value },
    GetConfig { module: ModuleId, name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandReply {
    Ok,
    Error { module: Option<ModuleId>, message: String },
    Data { module: Option<ModuleId>, value: Value },
}

impl CommandReply {
    pub fn new_error(module: Option<ModuleId>, message: String) -> Self {
        CommandReply::Error { module, message }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, CommandReply::Error { .. })
    }
}


pub struct CommandHandler {
    sock: net::UnixDatagram,
    mod_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
    post_send: Sender<PipeItem>,
}

impl CommandHandler {
    pub fn new(
        socket_name: &str,
        mod_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
        post_send: Sender<PipeItem>,
    ) -> UResult<Self> {
        let addr = uds::UnixSocketAddr::from_abstract(socket_name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = net::UnixDatagram::bind_unix_addr(&addr)
            .with_context(|| format!("Binding command listener to {}", addr))?;
        Ok(Self { sock, mod_send, post_send })
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
                CommandReply::new_error(None, "Invalid UTF-8 in telegram".into())
            }
            Ok(s) => match serde_json::from_str::<Command>(s) {
                Err(e) => {
                    lprintln!(ERROR, "Received invalid command {s:?}: {e:#}");
                    CommandReply::new_error(None, format!("Invalid JSON or invalid command: {e:#}"))
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
        let (rep_send, rep_recv) = crate::channel::bounded(self.mod_send.len());
        match cmd {
            Command::Ping => {
                let version = format!("UMAMI {}", env!("CARGO_PKG_VERSION"));
                CommandReply::Data { module: None, value: version.into() }
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
            Command::SetMode { name, params } => {
                self.post_send.send(PipeItem::SetMode(name, params, rep_send))
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
                for mod_send in self.mod_send.values() {
                    mod_send.send(cmd_and_send.clone()).expect("module command receiver died");
                }
                let replies = (0..self.mod_send.len()).map(
                    |_| rep_recv.recv().expect("module command sender died")
                ).collect_vec();
                if let Some(err) = replies.into_iter().find(|r| r.is_error()) {
                    return err;
                }
                CommandReply::Ok
            }
            Command::Config { module, .. } | Command::GetConfig { module, .. } => {
                if let Some(mod_send) = self.mod_send.get(&module) {
                    mod_send.send((cmd, rep_send)).expect("module command receiver died");
                    rep_recv.recv().expect("module command sender died")
                } else {
                    CommandReply::new_error(
                        Some(module),
                        format!("Module {} not found", module.0),
                    )
                }
            }
        }
    }
}
