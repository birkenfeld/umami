// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::io::Read;
use crate::lprintln;
use crate::channel::{Sender, Receiver};
use crate::event::{Event, ModuleId};
use crate::error::{UError, UResult};
use crate::config::ModuleConfig;
use crate::util::resolve;

#[derive(Clone)]
pub struct InputChannels {
    pub events: Sender<Vec<Event>>,
    pub command: Receiver<()>,
    pub state: Sender<()>,
    pub config_request: Receiver<()>,
    pub config_reply: Sender<()>,
}

pub fn init(module: ModuleId, config: ModuleConfig, channels: InputChannels) -> UResult<()> {
    Ok(match config {
        ModuleConfig::GE(cfg) => ge::GeInput::init(module, cfg, channels)?,
        ModuleConfig::Canon(cfg) => canon::CanonInput::init(module, cfg, channels)?,
        ModuleConfig::Mesy(cfg) => mesy::MesyInput::init(module, cfg, channels)?,
    })
}


pub trait Input: Send {
    fn read_events(&mut self) -> UResult<Option<Vec<Event>>>;
    fn description(&self) -> String;
    fn channels(&self) -> &InputChannels;

    fn start_event_thread(mut self) where Self: Sized + 'static {
        lprintln!(INFO, "Initialized {}", self.description());
        std::thread::spawn(move || loop {
            match self.read_events() {
                Ok(Some(ev)) => self.channels().events.send(ev).unwrap(),
                // TODO: what to do here?
                Err(e) => panic!("Failed to read events from {}: {}", self.description(), e),
                Ok(None) => {
                    lprintln!(INFO, "End of input from {}", self.description());
                    //self.channels().state.send(()).unwrap(); TODO these are not read ATM
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
        std::fs::File::open(cfg)
            .map_err(|e| UError::SourceInit(e))
    }

    fn description(&self) -> String {
        "<file>".into()
    }
}

impl Source for std::net::TcpStream {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        let addr = resolve(cfg)?;
        std::net::TcpStream::connect(addr)
            .map_err(|e| UError::SourceInit(e))
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
        std::net::UdpSocket::bind(addr)
            .map_err(|e| UError::SourceInit(e))
            .map(UdpReader)
    }

    fn description(&self) -> String {
        self.0.peer_addr().map(|x| x.to_string()).unwrap_or("?".into())
    }
}
