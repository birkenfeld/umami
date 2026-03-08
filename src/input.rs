// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::thread;
use std::io::{Read, Seek};
use std::time::Duration;
use anyhow::Context;
use crate::{lprintln, ltrace};
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

pub struct InputCommon {
    pub needs_reset: bool,
    pub running: bool,
    pub module: ModuleId,
    pub state: Sender<InputState>,
    pub events: Sender<Vec<Event>>,
    pub command: Receiver<Command>,
    pub command_reply: Sender<CommandReply>,
    pub recipe: Box<dyn Recipe>,
}

pub fn start(config: SpecificModuleConfig, common: InputCommon) -> UResult<()> {
    match config {
        SpecificModuleConfig::GE(cfg) => ge::GeInput::start(cfg, common)?,
        SpecificModuleConfig::Canon(cfg) => canon::CanonInput::start(cfg, common)?,
        SpecificModuleConfig::Mesy(cfg) => mesy::MesyInput::start(cfg, common)?,
    }
    Ok(())
}

pub trait Input: Send {
    fn description(&self) -> String;
    fn handle(&mut self, cmd: Command) -> UResult<CommandReply>;
    fn start(&mut self) -> UResult<()>;
    fn stop(&mut self) -> UResult<()>;
    fn read_events(&mut self) -> UResult<Option<Vec<Event>>>;

    // Rest of methods are all fully implemented

    fn start_main_loop(self, common: InputCommon) -> UResult<()>
    where Self: Sized + 'static
    {
        let desc = self.description();
        lprintln!(INFO, "Initialized {desc}");
        thread::Builder::new()
            .name(format!("input-{}", self.description()))
            .spawn(move || self.main_loop(common))
            .context(format!("Spawning input thread for {desc}"))?;
        Ok(())
    }

    fn main_loop_command(&mut self, cmd: Command, common: &mut InputCommon) {
        let mid = common.module;
        let reply = match cmd {
            Command::Start => {
                if let Err(e) = self.start() {
                    common.needs_reset = true;
                    common.state.send(InputState::Errored(mid)).expect("state channel closed");
                    CommandReply::new_error(
                        Some(mid), format!("Failed to start input: {}", e)
                    )
                } else {
                    common.needs_reset = false;
                    common.running = true;
                    common.state.send(InputState::Running(mid)).expect("state channel closed");
                    CommandReply::Ok
                }
            }
            Command::Stop => {
                common.running = false;
                common.events.send(vec![Event::end(mid)]).expect("event channel closed");
                if let Err(e) = self.stop() {
                    common.needs_reset = true;
                    common.state.send(InputState::Errored(mid)).expect("state channel closed");
                    CommandReply::new_error(
                        Some(mid), format!("Failed to stop input: {}", e)
                    )
                } else {
                    common.state.send(InputState::Stopped(mid)).expect("state channel closed");
                    CommandReply::Ok
                }
            }
            _ => match self.handle(cmd) {
                Ok(reply) => reply,
                Err(e) => CommandReply::new_error(Some(mid),
                                                  format!("Failed to handle command: {}", e)),
            }
        };
        common.command_reply.send(reply).expect("command channel closed");
    }

    fn main_loop(mut self, mut common: InputCommon)
    where Self: Sized
    {
        let desc = self.description();
        let mid = common.module;

        loop {
            match common.command.try_recv() {
                Ok(None) => (),
                Ok(Some(cmd)) => self.main_loop_command(cmd, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {}: {}, exiting input", desc, e);
                    return;
                }
            }

            if !common.needs_reset {
                match self.read_events() {
                    Ok(Some(ev)) => {
                        ltrace!("{} | Incoming events: {:?}", desc, ev);
                        if common.running {
                            let ev = common.recipe.process(ev);
                            ltrace!("{} | Processed events: {:?}", desc, ev);
                            common.events.send(ev).expect("event channel closed");
                        }
                        continue;
                    }
                    Err(e) => {
                        lprintln!(ERROR, "Cannot read events for {}: {}", desc, e);
                        common.needs_reset = true;
                        common.events.send(vec![Event::end(mid)]).expect("event channel closed");
                        common.state.send(InputState::Errored(mid)).expect("state channel closed");
                    }
                    Ok(None) => {
                        common.needs_reset = true;
                        common.events.send(vec![Event::end(mid)]).expect("event channel closed");
                        common.state.send(InputState::Ended(mid)).expect("state channel closed");
                        // wait for commands below
                    }
                }
            }

            // no events can be collected; wait for commands
            match common.command.recv() {
                Ok(cmd) => self.main_loop_command(cmd, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {}: {}, exiting input", desc, e);
                    return;
                }
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
