// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use crate::channel::{Sender, Receiver};
use crate::event::ModuleId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    to_module: Option<ModuleId>,
    command: CommandType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommandType {
    Start,
    Stop,
    Config(String, Value),
    GetConfig(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandReply {
    from_module: Option<ModuleId>,
    reply: CommandReplyType,
}

impl CommandReply {
    pub fn new(from_module: Option<ModuleId>, reply: CommandReplyType) -> Self {
        Self { from_module, reply }
    }

    pub fn new_ok(from_module: Option<ModuleId>) -> Self {
        Self::new(from_module, CommandReplyType::Ok)
    }

    // pub fn new_error(from_module: Option<ModuleId>, message: String) -> Self {
    //     Self::new(from_module, CommandReplyType::Error(message))
    // }

    pub fn is_error(&self) -> bool {
        matches!(self.reply, CommandReplyType::Error(_))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommandReplyType {
    Ok,
    Error(String),
    Data(Value),
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
            .name("CommandHandler".into())
            .spawn(move || handler.main())
            .context("Spawning command handler thread")?;
        Ok(())
    }

    fn main(&mut self) {
        while let Ok(cmd) = self.if_rcv.recv() {
            if let Some(mod_id) = cmd.to_module {
                // Command for a specific module
                if let Some(mod_send) = self.mod_send.get(&mod_id) {
                    mod_send.send(cmd).unwrap(); // TODO
                } else {
                    self.if_send.send(CommandReply {
                        from_module: Some(mod_id),
                        reply: CommandReplyType::Error(format!("Module {} not found", mod_id.0)),
                    }).unwrap();
                }
                if let Ok(reply) = self.mod_rcv.recv() {
                    self.if_send.send(reply).unwrap(); // TODO
                }
            } else {
                // Command for all modules
                for mod_send in self.mod_send.values() {
                    mod_send.send(cmd.clone()).unwrap(); // TODO
                }
                let replies = (0..self.mod_send.len())
                    .map(|_| self.mod_rcv.recv())
                    .collect::<Vec<_>>();
                // TODO
                if replies.iter().any(|r| r.is_err() || r.as_ref().unwrap().is_error()) {
                    self.if_send.send(CommandReply {
                        from_module: None,
                        reply: CommandReplyType::Error("One or more modules returned an error".into()),
                    }).unwrap();
                } else {
                    self.if_send.send(CommandReply {
                        from_module: None,
                        reply: CommandReplyType::Ok,
                    }).unwrap();
                }
            }
        }
    }
}
