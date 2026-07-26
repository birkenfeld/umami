// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::params::HasParams;
use super::Recipe;

/// Extracts the Jumiom PSD's raw FPGA X/Y position (position mode 0 -- see
/// `src/input/jumiom/decode.rs`) from the channel encoding the `jumiom`
/// input produces for Neutron events: X in the low byte, Y in the next
/// byte. Any future ADC-ratio/distortion-correction position mode or
/// per-pixel limit-table filtering belongs here, not in the input.
#[derive(HasParams)]
pub struct Position {}

impl Recipe for Position {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(Position {})
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            if let EventType::Neutron = event.evtype {
                event.histo.x = (event.channel.0 & 0xFF) as u16;
                event.histo.y = ((event.channel.0 >> 8) & 0xFF) as u16;
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;

    #[test]
    fn test_jumiom_position_extracts_x_y_from_channel() {
        let mut recipe = Position::from_config(toml::Table::new(), &BTreeMap::new()).unwrap();
        let channel = (3u32 << 8) | 5u32; // y=3, x=5
        let ev = test_utils::neutron(100, channel);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 5);
        assert_eq!(out[0].histo.y, 3);
    }

    #[test]
    fn test_jumiom_position_leaves_other_event_types_unchanged() {
        let mut recipe = Position::from_config(toml::Table::new(), &BTreeMap::new()).unwrap();
        let ev = test_utils::tzero(50);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Tzero);
        assert_eq!(out[0].histo.x, 0);
        assert_eq!(out[0].histo.y, 0);
    }
}
