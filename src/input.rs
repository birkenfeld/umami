// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::fs::File;
use std::thread;
use std::io::{Seek, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;
use anyhow::Context;
use serde::Serialize;
use crate::{lprintln, ltrace};
use crate::channel::{Sender, Receiver, TryRecvError};
use crate::command::{Command, CommandReply};
use crate::config::SpecificModuleConfig;
use crate::error::{UError, UResult};
use crate::event::{Event, ModuleId};
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;
use crate::util::resolve;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleState {
    Idle,
    Running,
    Ended,
    Error(String),
}

pub struct ModuleCommon {
    state: ModuleState,
    module: ModuleId,
    state_send: Sender<PipeItem>,
    events: Sender<PipeItem>,
    command: Receiver<(Command, Sender<CommandReply>)>,
    recipe: Box<dyn Recipe>,
}

impl ModuleCommon {
    pub fn new(
        module: ModuleId,
        state_send: Sender<PipeItem>,
        events: Sender<PipeItem>,
        command: Receiver<(Command, Sender<CommandReply>)>,
        recipe: Box<dyn Recipe>,
    ) -> Self {
        Self {
            state: ModuleState::Idle,
            module,
            state_send,
            events,
            command,
            recipe,
        }
    }

    fn set_state(&mut self, state: ModuleState) {
        if self.state == ModuleState::Running {
            self.events.send(PipeItem::EndOfRun).expect("event channel closed");
        }
        if let ModuleState::Error(e) = &state {
            lprintln!(ERROR, "Module {} entered error state: {}", self.module.0, e);
        }
        self.state = state.clone();
        self.state_send.send(PipeItem::ModuleState(self.module, state))
                       .expect("state channel closed");
    }
}

pub fn start(config: SpecificModuleConfig, confdir: &Path, common: ModuleCommon) -> UResult<()> {
    match config {
        SpecificModuleConfig::GE(cfg) => ge::GeModule::start(cfg, confdir, common)?,
        SpecificModuleConfig::Canon(cfg) => canon::CanonModule::start(cfg, confdir, common)?,
        SpecificModuleConfig::Mesy(cfg) => mesy::MesyModule::start(cfg, confdir, common)?,
    }
    Ok(())
}

pub trait Module: Send {
    fn description(&self) -> String;
    fn handle(&mut self, cmd: Command) -> UResult<CommandReply>;
    fn start(&mut self, run_id: String) -> UResult<()>;
    fn stop(&mut self) -> UResult<()>;
    fn reset(&mut self) -> UResult<()>;
    fn read_events(&mut self) -> UResult<Vec<Event>>;

    // Rest of methods are all fully implemented

    fn start_main_loop(self, common: ModuleCommon) -> UResult<()>
    where Self: Sized + 'static
    {
        let desc = self.description();
        lprintln!(INFO, "Initialized {desc}");
        thread::Builder::new()
            .name(format!("Module {}", self.description()))
            .spawn(move || self.main_loop(common))
            .context(format!("Spawning module thread for {desc}"))?;
        Ok(())
    }

    fn main_loop_command(&mut self, cmd: Command, rep: Sender<CommandReply>, common: &mut ModuleCommon) {
        let mid = common.module;
        let reply = match cmd {
            Command::Start { run_id } => match &common.state {
                ModuleState::Error(e) => {
                    CommandReply::new_error(
                        Some(mid),
                        format!("Cannot start module in error state. Last error: {e:#}")
                    )
                }
                _ => {
                    if common.state == ModuleState::Running {
                        if let Err(e) = self.stop() {
                            let msg = format!("Failed to stop module for restart: {e:#}");
                            common.set_state(ModuleState::Error(msg.clone()));
                            rep.send(CommandReply::new_error(Some(mid), msg))
                               .expect("command channel closed");
                            return;
                        } else {
                            common.set_state(ModuleState::Idle);
                        }
                    }
                    if let Err(e) = self.start(run_id) {
                        let msg = format!("Failed to start module: {e:#}");
                        common.set_state(ModuleState::Error(msg.clone()));
                        CommandReply::new_error(Some(mid), msg)
                    } else {
                        common.set_state(ModuleState::Running);
                        CommandReply::Ok
                    }
                }
            }
            Command::Stop => match &common.state {
                ModuleState::Error(e) => {
                    CommandReply::new_error(
                        Some(mid),
                        format!("Cannot stop module in error state. Last error: {e:#}")
                    )
                }
                ModuleState::Idle => {
                    CommandReply::Ok
                }
                ModuleState::Ended => {
                    common.set_state(ModuleState::Idle);
                    CommandReply::Ok
                }
                ModuleState::Running => {
                    if let Err(e) = self.stop() {
                        let msg = format!("Failed to stop module: {e:#}");
                        common.set_state(ModuleState::Error(msg.clone()));
                        CommandReply::new_error(Some(mid), msg)
                    } else {
                        common.set_state(ModuleState::Idle);
                        CommandReply::Ok
                    }
                }
            }
            Command::Reset => match common.state {
                ModuleState::Error(_) => {
                    if let Err(e) = self.reset() {
                        let msg = format!("Failed to reset module: {e:#}");
                        common.set_state(ModuleState::Error(msg.clone()));
                        CommandReply::new_error(Some(mid), msg)
                    } else {
                        lprintln!(INFO, "Reset {}", self.description());
                        common.set_state(ModuleState::Idle);
                        CommandReply::Ok
                    }
                }
                _ => CommandReply::Ok
            }
            _ => match self.handle(cmd) {
                Ok(reply) => reply,
                Err(e) => CommandReply::new_error(Some(mid),
                                                  format!("Failed to handle command: {e:#}")),
            }
        };
        rep.send(reply).expect("command channel closed");
    }

    fn main_loop(mut self, mut common: ModuleCommon)
    where Self: Sized
    {
        let desc = self.description();
        common.set_state(ModuleState::Idle);

        loop {
            match common.command.try_recv() {
                Err(TryRecvError::Empty) => (),
                Ok((cmd, rep)) => self.main_loop_command(cmd, rep, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {desc}: {e:#}, exiting");
                    return;
                }
            }

            if !matches!(common.state, ModuleState::Error(_)) {
                match self.read_events() {
                    Ok(ev) => {
                        ltrace!("{} | Incoming events: {:?}", desc, ev);
                        if common.state == ModuleState::Running {
                            let ev = common.recipe.process(ev);
                            ltrace!("{} | Processed events: {:?}", desc, ev);
                            common.events.send(PipeItem::Events(ev)).expect("event channel closed");
                        }
                        continue;
                    }
                    Err(UError::Other(e)) => {
                        let msg = format!("Error reading events: {e:#}");
                        lprintln!(ERROR, "Cannot read events for {desc}: {e:#}");
                        common.set_state(ModuleState::Error(msg));
                        // wait for commands below
                    }
                    Err(UError::NoMoreData) => {
                        common.set_state(ModuleState::Ended);
                        // wait for commands below
                    }
                }
            }

            // no events can be collected; wait for commands
            match common.command.recv() {
                Ok((cmd, rep)) => self.main_loop_command(cmd, rep, &mut common),
                Err(e) => {
                    lprintln!(ERROR, "Cannot read command for {desc}: {e:#}, exiting");
                    return;
                }
            }
        }
    }
}


pub trait Source: Send + 'static {
    type Config;
    fn from_config(cfg: &Self::Config, confdir: &Path) -> UResult<Self> where Self: Sized;
    fn description(&self) -> String;
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()>;
    fn reset(&mut self) -> UResult<()>;
    fn rewind(&mut self) -> UResult<()> {
        Ok(())
    }
}

pub struct ReplayFile {
    file: std::fs::File,
    name: String,
}

impl Source for ReplayFile {
    type Config = String;

    fn from_config(cfg: &Self::Config, confdir: &Path) -> UResult<Self> {
        let file = std::fs::File::open(confdir.join(cfg))
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
        self.rewind()
    }

    fn rewind(&mut self) -> UResult<()> {
        self.file
            .seek(std::io::SeekFrom::Start(0))
            .context("Resetting file source")?;
        Ok(())
    }
}

impl Source for std::net::TcpStream {
    type Config = String;

    fn from_config(cfg: &Self::Config, _: &Path) -> UResult<Self> {
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

    fn reset(&mut self) -> UResult<()> {
        let addr = self.peer_addr()
            .context("Getting previous peer address")?;
        *self = std::net::TcpStream::connect(addr)
            .with_context(|| format!("Connecting to {}", addr))?;
        self.set_read_timeout(Some(Duration::from_millis(300)))
            .context("Setting socket timeout")?; // TODO configurable?
        Ok(())
    }
}

pub struct UdpReader(std::net::UdpSocket, std::net::SocketAddr);

impl std::io::Read for UdpReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Source for UdpReader {
    type Config = String;

    fn from_config(cfg: &Self::Config, _: &Path) -> UResult<Self> {
        let addr = resolve(cfg)?;
        let sock = std::net::UdpSocket::bind(addr)
            .context(format!("Binding to source socket {}", addr))?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))
            .context("Setting socket timeout")?; // TODO configurable?
        Ok(UdpReader(sock, addr))
    }

    fn description(&self) -> String {
        format!("local addr {}", self.1)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        std::io::Read::read_exact(self, buf)
    }

    fn reset(&mut self) -> UResult<()> {
        // Close first, cannot bind the new socket and *then* move it in place
        let _ = nix::unistd::close(self.0.as_raw_fd());
        let new_sock = std::net::UdpSocket::bind(self.1)
            .context(format!("Rebinding to source socket {}", self.1))?;
        new_sock.set_read_timeout(Some(Duration::from_millis(300)))
            .context("Setting socket timeout")?; // TODO configurable?
        self.0 = new_sock;
        Ok(())
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
