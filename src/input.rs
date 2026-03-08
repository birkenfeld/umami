// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::io::Read;
use anyhow::Context;
use crate::lprintln;
use crate::channel::{Sender, Receiver};
use crate::command::{Command, CommandReply};
use crate::config::SpecificModuleConfig;
use crate::error::UResult;
use crate::event::{Event, ModuleId};
use crate::recipe::Recipe;
use crate::util::resolve;

#[derive(Debug)]
pub enum InputState {
    Running(ModuleId),
    Ended(ModuleId),
}

pub struct InputPlumbing {
    pub state: Sender<InputState>,
    pub events: Sender<Vec<Event>>,
    pub command: Receiver<Command>,
    pub command_reply: Sender<CommandReply>,
    pub recipe: Box<dyn Recipe>,
}

pub fn start(module: ModuleId, config: SpecificModuleConfig, plumbing: InputPlumbing) -> UResult<()> {
    Ok(match config {
        SpecificModuleConfig::GE(cfg) => ge::GeInput::start(module, cfg, plumbing)?,
        SpecificModuleConfig::Canon(cfg) => canon::CanonInput::start(module, cfg, plumbing)?,
        SpecificModuleConfig::Mesy(cfg) => mesy::MesyInput::start(module, cfg, plumbing)?,
    })
}

pub trait InputCmdHandler: Send {
    fn handle(&mut self, cmd: Command) -> CommandReply;
}


pub trait Input: Send {
    type CmdHandler: InputCmdHandler;

    fn description(&self) -> String;
    fn command_handler(&self, plumbing: &InputPlumbing) -> Self::CmdHandler;

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>>;

    fn start(mut self, module: ModuleId, mut plumbing: InputPlumbing) where Self: Sized + 'static {
        lprintln!(INFO, "Initialized {}", self.description());

        let mut cmd_handler = self.command_handler(&plumbing);
        let desc = self.description();
        std::thread::spawn(move || loop {
            match plumbing.command.recv() {
                Ok(cmd) => {
                    let reply = cmd_handler.handle(cmd);
                    plumbing.command_reply.send(reply).unwrap(); // TODO
                }
                // TODO
                Err(e) => panic!("Failed to read command for {}: {}", desc, e),
            }
        });

        std::thread::spawn(move || loop {
            match self.read_events() {
                Ok(Some(ev)) => {
                    let ev = plumbing.recipe.process(ev);
                    plumbing.events.send(ev).unwrap();
                }
                // TODO: what to do here?
                Err(e) => panic!("Failed to read events from {}: {}", self.description(), e),
                Ok(None) => {
                    lprintln!(INFO, "End of input from {}", self.description());
                    plumbing.state.send(InputState::Ended(module)).unwrap(); // TODO
                    break;
                }
            }
        });
    }
}


pub trait Source: Read + Send + 'static {
    type Config;
    fn from_config(cfg: &Self::Config) -> UResult<Self> where Self: Sized;
    fn description(&self) -> String;
}

impl Source for std::fs::File {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        Ok(std::fs::File::open(cfg)
           .with_context(|| format!("Opening source file {:?}", cfg))?)
    }

    fn description(&self) -> String {
        "<file>".into()
    }
}

impl Source for std::net::TcpStream {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        let addr = resolve(cfg)?;
        Ok(std::net::TcpStream::connect(addr)
           .with_context(|| format!("Connecting to source socket {}", addr))?)
    }

    fn description(&self) -> String {
        self.peer_addr().map(|x| x.to_string()).unwrap_or("?".into())
    }
}

pub struct UdpReader(std::net::UdpSocket);

impl std::io::Read for UdpReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Source for UdpReader {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        let addr = resolve(cfg)?;
        Ok(UdpReader(
            std::net::UdpSocket::bind(addr)
                .context(format!("Binding to source socket {}", addr))?
        ))
    }

    fn description(&self) -> String {
        self.0.peer_addr().map(|x| x.to_string()).unwrap_or("?".into())
    }
}


pub struct NullCmdHandler(pub ModuleId);

impl InputCmdHandler for NullCmdHandler {
    fn handle(&mut self, _cmd: Command) -> CommandReply {
        CommandReply::new_ok(Some(self.0))
    }
}
