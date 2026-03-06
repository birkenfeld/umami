// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::io::Read;
use anyhow::Context;
use crate::lprintln;
use crate::channel::{Sender, Receiver};
use crate::event::{Event, ModuleId};
use crate::error::UResult;
use crate::config::SpecificModuleConfig;
use crate::recipe::Recipe;
use crate::util::resolve;

pub struct InputPlumbing {
    pub events: Sender<Vec<Event>>,
    pub command: Receiver<()>,
    pub state: Sender<()>,
    pub config_request: Receiver<()>,
    pub config_reply: Sender<()>,
    pub recipe: Box<dyn Recipe>,
}

pub fn init(module: ModuleId, config: SpecificModuleConfig, plumbing: InputPlumbing) -> UResult<()> {
    Ok(match config {
        SpecificModuleConfig::GE(cfg) => ge::GeInput::init(module, cfg, plumbing)?,
        SpecificModuleConfig::Canon(cfg) => canon::CanonInput::init(module, cfg, plumbing)?,
        SpecificModuleConfig::Mesy(cfg) => mesy::MesyInput::init(module, cfg, plumbing)?,
    })
}


pub trait Input: Send {
    fn read_events(&mut self) -> UResult<Option<Vec<Event>>>;
    fn description(&self) -> String;
    fn plumbing(&self) -> &InputPlumbing;

    fn start_event_thread(mut self) where Self: Sized + 'static {
        lprintln!(INFO, "Initialized {}", self.description());
        std::thread::spawn(move || loop {
            match self.read_events() {
                Ok(Some(ev)) => self.plumbing().events.send(ev).unwrap(),
                // TODO: what to do here?
                Err(e) => panic!("Failed to read events from {}: {}", self.description(), e),
                Ok(None) => {
                    lprintln!(INFO, "End of input from {}", self.description());
                    //self.plumbing().state.send(()).unwrap(); TODO these are not read ATM
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
