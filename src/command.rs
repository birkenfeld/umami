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
use crate::pipeline::PipeItem;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Clear,
    Start { run_id: String },
    Stop,
    SetRawDump { enable: bool, path: String },
    SetMode { name: String, params: toml::Table },
    GetModes,
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
    if_recv: Receiver<Command>,
    mod_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
    post_send: Sender<PipeItem>,
    if_send: Sender<CommandReply>,
}

impl CommandHandler {
    pub fn new(
        if_recv: Receiver<Command>,
        mod_send: BTreeMap<ModuleId, Sender<(Command, Sender<CommandReply>)>>,
        if_send: Sender<CommandReply>,
        post_send: Sender<PipeItem>,
    ) -> Self {
        Self { if_recv, mod_send, if_send, post_send }
    }

    pub fn start(mut self) -> anyhow::Result<()> {
        std::thread::Builder::new()
            .name("Command handler".into())
            .spawn(move || self.main())
            .context("Spawning command handler thread")?;
        Ok(())
    }

    fn main(&mut self) {
        while let Ok(cmd) = self.if_recv.recv() {
            let reply = match self.handle(cmd) {
                Ok(reply) => reply,
                Err(e) => CommandReply::new_error(None, format!("Failed to handle command: {}", e)),
            };
            self.if_send.send(reply).expect("interface reply receiver died");
        }
    }

    pub fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        let (rep_send, rep_recv) = crate::channel::bounded(self.mod_send.len());
        match cmd {
            Command::Clear => {
                self.post_send.send(PipeItem::Clear)
                              .expect("postprocessor command receiver died");
                Ok(CommandReply::Ok)
            }
            Command::SetMode { name, params } => {
                self.post_send.send(PipeItem::SetMode(name, params, rep_send))
                              .expect("postprocessor command receiver died");
                Ok(rep_recv.recv().expect("postprocessor command sender died"))
            }
            Command::GetModes => {
                self.post_send.send(PipeItem::GetModes(rep_send))
                              .expect("postprocessor command receiver died");
                Ok(rep_recv.recv().expect("postprocessor command sender died"))
            }
            Command::Start { .. } | Command::Stop | Command::SetRawDump { .. } => {
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
                    return Ok(err);
                }
                Ok(CommandReply::Ok)
            }
            Command::Config { module, .. } | Command::GetConfig { module, .. } => {
                if let Some(mod_send) = self.mod_send.get(&module) {
                    mod_send.send((cmd, rep_send)).expect("module command receiver died");
                    let reply = rep_recv.recv().expect("module command sender died");
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
