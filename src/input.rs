// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::fs::File;
use std::thread;
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::time::Duration;
use anyhow::Context;
use crate::{lprintln, ltrace};
use crate::channel::{Sender, Receiver, TryRecvError};
use crate::command::{Command, CommandReply};
use crate::config::SpecificModuleConfig;
use crate::error::{UError, UResult};
use crate::event::{Event, ModuleId};
use crate::pipeline::PipeItem;
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
    pub state: Sender<PipeItem>,
    pub events: Sender<PipeItem>,
    pub command: Receiver<(Command, Sender<CommandReply>)>,
    pub recipe: Box<dyn Recipe>,
}

impl InputCommon {
    fn update_state(&self, state: InputState) {
        self.state.send(PipeItem::State(state))
                  .expect("state channel closed");
    }
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
    fn start(&mut self, run_id: String) -> UResult<()>;
    fn stop(&mut self) -> UResult<()>;
    fn read_events(&mut self) -> UResult<Vec<Event>>;

    // Rest of methods are all fully implemented

    fn start_main_loop(self, common: InputCommon) -> UResult<()>
    where Self: Sized + 'static
    {
        let desc = self.description();
        lprintln!(INFO, "Initialized {desc}");
        thread::Builder::new()
            .name(format!("Input {}", self.description()))
            .spawn(move || self.main_loop(common))
            .context(format!("Spawning input thread for {desc}"))?;
        Ok(())
    }

    fn main_loop_command(&mut self, cmd: Command, rep: Sender<CommandReply>, common: &mut InputCommon) {
        let mid = common.module;
        let reply = match cmd {
            Command::Start { run_id } => {
                if common.running {
                    if let Err(e) = self.stop() {
                        common.needs_reset = true;
                        common.update_state(InputState::Errored(mid));
                        rep.send(CommandReply::new_error(
                            Some(mid), format!("Failed to stop input for restart: {}", e)
                        )).expect("command channel closed");
                        return;
                    } else {
                        common.running = false;
                    }
                }

                if let Err(e) = self.start(run_id) {
                    common.needs_reset = true;
                    common.update_state(InputState::Errored(mid));
                    CommandReply::new_error(
                        Some(mid), format!("Failed to start input: {}", e)
                    )
                } else {
                    common.needs_reset = false;
                    common.running = true;
                    common.update_state(InputState::Running(mid));
                    CommandReply::Ok
                }
            }
            Command::Stop => {
                common.running = false;
                common.events.send(PipeItem::EndOfRun).expect("event channel closed");
                if let Err(e) = self.stop() {
                    common.needs_reset = true;
                    common.update_state(InputState::Errored(mid));
                    CommandReply::new_error(
                        Some(mid), format!("Failed to stop input: {}", e)
                    )
                } else {
                    common.update_state(InputState::Stopped(mid));
                    CommandReply::Ok
                }
            }
            _ => match self.handle(cmd) {
                Ok(reply) => reply,
                Err(e) => CommandReply::new_error(Some(mid),
                                                  format!("Failed to handle command: {}", e)),
            }
        };
        rep.send(reply).expect("command channel closed");
    }

    fn main_loop(mut self, mut common: InputCommon)
    where Self: Sized
    {
        let desc = self.description();
        let mid = common.module;

        loop {
            match common.command.try_recv() {
                Err(TryRecvError::Empty) => (),
                Ok((cmd, rep)) => self.main_loop_command(cmd, rep, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {}: {}, exiting input", desc, e);
                    return;
                }
            }

            if !common.needs_reset {
                match self.read_events() {
                    Ok(ev) => {
                        ltrace!("{} | Incoming events: {:?}", desc, ev);
                        if common.running {
                            let ev = common.recipe.process(ev);
                            ltrace!("{} | Processed events: {:?}", desc, ev);
                            common.events.send(PipeItem::Events(ev)).expect("event channel closed");
                        }
                        continue;
                    }
                    Err(UError::Other(e)) => {
                        lprintln!(ERROR, "Cannot read events for {}: {}", desc, e);
                        common.needs_reset = true;
                        common.events.send(PipeItem::EndOfRun).expect("event channel closed");
                        common.update_state(InputState::Errored(mid));
                    }
                    Err(UError::InputEnded) => {
                        common.needs_reset = true;
                        common.events.send(PipeItem::EndOfRun).expect("event channel closed");
                        common.update_state(InputState::Ended(mid));
                        // wait for commands below
                    }
                }
            }

            // no events can be collected; wait for commands
            match common.command.recv() {
                Ok((cmd, rep)) => self.main_loop_command(cmd, rep, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {}: {}, exiting input", desc, e);
                    return;
                }
            }
        }
    }
}


pub trait Source: Send + 'static {
    type Config;
    fn from_config(cfg: &Self::Config) -> UResult<Self> where Self: Sized;
    fn description(&self) -> String;
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()>;
    fn reset(&mut self) -> UResult<()> {
        Ok(())
    }
}

pub struct ReplayFile {
    file: std::fs::File,
    name: String,
}

impl Source for ReplayFile {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        let file = std::fs::File::open(cfg)
           .with_context(|| format!("Opening source file {:?}", cfg))?;
        Ok(Self {
            file,
            name: cfg.clone(),
        })
    }

    fn description(&self) -> String {
        format!("{:?}", self.name)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        std::io::Read::read_exact(&mut self.file, buf)
    }

    fn reset(&mut self) -> UResult<()> {
        self.file
            .seek(std::io::SeekFrom::Start(0))
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

    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        std::io::Read::read_exact(self, buf)
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

    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        std::io::Read::read_exact(self, buf)
    }
}


#[derive(Debug, Default)]
pub struct DumpHandler {
    path: Option<PathBuf>,
    file: Option<File>,
}

impl DumpHandler {
    pub fn configure(&mut self, enable: bool, path: String) -> UResult<()> {
        if enable {
            self.path = Some(PathBuf::from(path));
        } else {
            self.path = None;
            self.file = None;
        }
        Ok(())
    }

    pub fn start(&mut self, module: ModuleId, run_id: &str) -> UResult<()> {
        if let Some(path) = &self.path {
            let full_path = path.join(run_id);
            std::fs::create_dir_all(&full_path).context("Creating raw data directory")?;
            let file_name = full_path.join(format!("{:02}", module.0));
            let raw_file = File::create(file_name).context("Creating raw data file")?;
            self.file = Some(raw_file);
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        self.file = None;
    }

    pub fn write(&mut self, data: &[u8]) -> UResult<()> {
        if let Some(file) = &mut self.file {
            file.write_all(data).context("Writing to raw dump file")?;
        }
        Ok(())
    }
}
