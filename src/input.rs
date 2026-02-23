// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod ge;

use crate::event::{Event, ModuleId};
use crate::error::UResult;
use crate::config::InputConfig;

pub trait Input : Send {
    fn read_event(&mut self) -> UResult<Event>;
}

pub fn create_input(module: ModuleId, config: InputConfig) -> UResult<Box<dyn Input>> {
    Ok(match config {
        InputConfig::GE { addr, ts } => Box::new(ge::GeInput::new(module, addr, ts)?)
    })
}
