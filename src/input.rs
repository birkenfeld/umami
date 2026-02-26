// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use std::io::Read;
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


pub trait Source: Read {
    type Config;
    fn from_config(cfg: &Self::Config) -> UResult<Self> where Self: Sized;
    fn description(&self) -> String;
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

impl Source for std::fs::File {
    type Config = String;

    fn from_config(cfg: &Self::Config) -> UResult<Self> {
        std::fs::File::open(cfg)
            .map_err(|e| UError::SourceInit(e))
    }

    fn description(&self) -> String {
        "file".into()
    }
}
