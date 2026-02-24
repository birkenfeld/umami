// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;

use crate::event::{Event, ModuleId};
use crate::error::UResult;
use crate::config::ModuleConfig;

pub trait Input : Send {
    fn description(&self) -> String;

    // TODO: instead possibly return list/vector of events?
    fn read_event(&mut self) -> UResult<Event>;
}

pub fn create_input(module: ModuleId, config: ModuleConfig) -> UResult<Box<dyn Input>> {
    Ok(match config {
        ModuleConfig::GE { addr, ts } => Box::new(ge::GeInput::new(module, addr, ts)?)
    })
}
