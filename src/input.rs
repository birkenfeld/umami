// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::thread;
use std::io::{Read, Seek};
use std::time::Duration;
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
#[allow(dead_code)]
pub enum InputState {
    Running(ModuleId),
    Stopped(ModuleId),
    Errored(ModuleId),
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
    match config {
        SpecificModuleConfig::GE(cfg) => ge::GeInput::start(module, cfg, plumbing)?,
        SpecificModuleConfig::Canon(cfg) => canon::CanonInput::start(module, cfg, plumbing)?,
        SpecificModuleConfig::Mesy(cfg) => mesy::MesyInput::start(module, cfg, plumbing)?,
    }
    Ok(())
}

pub trait Input: Send {
    fn description(&self) -> String;
    fn handle(&mut self, cmd: Command) -> CommandReply;
    fn start(&mut self) -> UResult<()>;
    fn stop(&mut self) -> UResult<()>;
    fn read_events(&mut self) -> UResult<Option<Vec<Event>>>;

    fn start_main_loop(self, module: ModuleId, plumbing: InputPlumbing) -> UResult<()>
    where Self: Sized + 'static
    {
        let desc = self.description();
        lprintln!(INFO, "Initialized {desc}");
        thread::Builder::new()
            .name(format!("input-{}", self.description()))
            .spawn(move || main_loop(self, module, plumbing))
            .context(format!("Spawning input thread for {desc}"))?;
        Ok(())
    }
}

fn main_loop(mut input: impl Input, module: ModuleId, mut plumbing: InputPlumbing) {
    let desc = input.description();
    let mut running = false;

    loop {
        match plumbing.command.try_recv() {
            Ok(None) => (),
            Ok(Some(cmd)) => {
                match cmd {
                    Command::AutoStart => {
                        running = true;
                        let _ = input.start();
                        plumbing.state.send(InputState::Running(module)).unwrap(); // TODO
                    }
                    Command::Start => {
                        running = true;
                        if let Err(e) = input.start() {
                            plumbing.state.send(InputState::Errored(module)).unwrap(); // TODO
                            plumbing.command_reply.send(CommandReply::new_error(
                                Some(module), format!("Failed to start input: {}", e)
                            )).unwrap(); // TODO
                        } else {
                            plumbing.state.send(InputState::Running(module)).unwrap(); // TODO
                            plumbing.command_reply.send(CommandReply::Ok).unwrap(); // TODO
                        }
                    }
                    Command::Stop => {
                        running = false;
                        if let Err(e) = input.stop() {
                            plumbing.state.send(InputState::Errored(module)).unwrap(); // TODO
                            plumbing.command_reply.send(CommandReply::new_error(
                                Some(module), format!("Failed to stop input: {}", e)
                            )).unwrap(); // TODO
                        } else {
                            plumbing.state.send(InputState::Stopped(module)).unwrap(); // TODO
                            plumbing.command_reply.send(CommandReply::Ok).unwrap(); // TODO
                        }
                    }
                    _ => {
                        let reply = input.handle(cmd);
                        plumbing.command_reply.send(reply).unwrap(); // TODO
                    }
                }
            }
            // TODO
            Err(e) => panic!("Failed to read command for {}: {}", desc, e),
        }

        match input.read_events() {
            Ok(Some(ev)) => {
                if running {
                    let ev = plumbing.recipe.process(ev);
                    plumbing.events.send(ev).unwrap(); // TODO
                }
            }
            // TODO: what to do here?
            Err(e) => panic!("Failed to read events from {}: {}", input.description(), e),
            Ok(None) => {
                plumbing.state.send(InputState::Ended(module)).expect("state channel closed");
                plumbing.command.peek();
            }
        }
    }
}


pub trait Source: Read + Send + 'static {
    type Config;
    fn from_config(cfg: &Self::Config) -> UResult<Self> where Self: Sized;
    fn description(&self) -> String;
    fn reset(&mut self) -> UResult<()> {
        Ok(())
    }
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

    fn reset(&mut self) -> UResult<()> {
        self.seek(std::io::SeekFrom::Start(0))
            .context("Resetting file source")?;
        Ok(())
    }
}

impl Source for std::net::TcpStream {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        let addr = resolve(cfg)?;
        let stream = std::net::TcpStream::connect(addr)
            .with_context(|| format!("Connecting to {}", addr))?;
        stream.set_read_timeout(Some(Duration::from_millis(300)))
            .context("Setting socket timeout")?; // TODO configurable?
        Ok(stream)
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
        let sock = std::net::UdpSocket::bind(addr)
            .context(format!("Binding to source socket {}", addr))?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))
            .context("Setting socket timeout")?; // TODO configurable?
        Ok(UdpReader(sock))
    }

    fn description(&self) -> String {
        self.0.peer_addr().map(|x| x.to_string()).unwrap_or("?".into())
    }
}
