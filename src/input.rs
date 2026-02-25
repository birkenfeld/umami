// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;
mod canon;
mod mesy;

use kanal;

use crate::event::{Event, ModuleId};
use crate::error::UResult;
use crate::config::ModuleConfig;

#[derive(Clone)]
pub struct InputChannels {
    pub events: kanal::Sender<Event>,
    pub command: kanal::Receiver<()>,
    pub config_request: kanal::Receiver<()>,
    pub config_reply: kanal::Sender<()>,
}

pub fn init(module: ModuleId, config: ModuleConfig, channels: InputChannels) -> UResult<()> {
    Ok(match config {
        ModuleConfig::GE(cfg) => ge::GeInput::init(module, cfg, channels)?,
        ModuleConfig::Canon(cfg) => canon::CanonInput::init(module, cfg, channels)?,
        ModuleConfig::Mesy(cfg) => mesy::MesyInput::init(module, cfg, channels)?,
    })
}
