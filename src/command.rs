// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use itertools::Itertools;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use crate::channel::{Sender, Receiver};
use crate::error::UResult;
use crate::event::ModuleId;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Clear,
    Start { run_id: String },
    Stop,
    SetRawDump { enable: bool, path: String },
    SetTofParams { nt: usize, dt: f64, t0: f64 },
    Config { module: ModuleId, name: String, value: Value },
    GetConfig { module: ModuleId, name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum CommandReply {
    Ok,
    Error { module: Option<ModuleId>, message: String },
    Data { module: ModuleId, value: Value },
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
    if_rcv: Receiver<Command>,
    mod_send: BTreeMap<ModuleId, Sender<Command>>,
    mod_rcv: Receiver<CommandReply>,
    if_send: Sender<CommandReply>,
}

impl CommandHandler {
    pub fn start(if_rcv: Receiver<Command>,
                 mod_send: BTreeMap<ModuleId, Sender<Command>>,
                 mod_rcv: Receiver<CommandReply>,
                 if_send: Sender<CommandReply>) -> anyhow::Result<()> {
        let mut handler = CommandHandler { if_rcv, mod_send, mod_rcv, if_send };
        std::thread::Builder::new()
            .name("Command handler".into())
            .spawn(move || handler.main())
            .context("Spawning command handler thread")?;
        Ok(())
    }

    fn main(&mut self) {
        while let Ok(cmd) = self.if_rcv.recv() {
            let reply = match self.handle(cmd) {
                Ok(reply) => reply,
                Err(e) => CommandReply::new_error(None, format!("Failed to handle command: {}", e)),
            };
            self.if_send.send(reply).expect("interface reply receiver died");
        }
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        match cmd {
            Command::Clear => {
                // TODO implement
                Ok(CommandReply::Ok)
            }
            Command::SetTofParams { .. } => {
                // TODO implement
                Ok(CommandReply::Ok)
            }
            Command::Start { .. } | Command::Stop | Command::SetRawDump { .. } => {
                for mod_send in self.mod_send.values() {
                    mod_send.send(cmd.clone()).expect("module command receiver died");
                }
                let replies = (0..self.mod_send.len()).map(
                    |_| self.mod_rcv.recv().expect("module command sender died")
                ).collect_vec();
                if let Some(err) = replies.into_iter().find(|r| r.is_error()) {
                    return Ok(err);
                }
                Ok(CommandReply::Ok)
            }
            Command::Config { module, .. } | Command::GetConfig { module, .. } => {
                if let Some(mod_send) = self.mod_send.get(&module) {
                    mod_send.send(cmd).expect("module command receiver died");
                    let reply = self.mod_rcv.recv().expect("module command sender died");
                    Ok(reply)
                } else {
                    Ok(CommandReply::new_error(
                        Some(module),
                        format!("Module {} not found", module.0),
                    ))
                }
            }
        }
    }
}
