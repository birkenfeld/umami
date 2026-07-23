// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::params::HasParams;
use super::Recipe;

// TODO: amplitude modes

#[derive(Debug, Deserialize, Clone, HasParams)]
pub struct Mpsd {}

impl Recipe for Mpsd {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(Self {})
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    let x_orig = event.channel.0;
                    // since the MPSD has only 8 channels per board,
                    // remove fourth bit of channel
                    let x = (x_orig >> 1) & 0xFFF8 | (x_orig & 0x7);
                    event.histo.x = x as u16;
                    event.histo.y = event.raw.0 as u16; // TODO: calibration
                }
                EventType::Edge { up: true } => {
                    event.evtype = EventType::Tzero;
                }
                _ => ()
            }
        }
        events
    }
}

#[derive(Debug, Deserialize, Clone, HasParams)]
pub struct Mdll {}

impl Recipe for Mdll {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(Self {})
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    event.histo.x = 0; // TODO
                    event.histo.y = 0;
                }
                EventType::Edge { up: true } => {
                    event.evtype = EventType::Tzero;
                }
                _ => ()
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;

    fn empty_recipes() -> BTreeMap<String, RecipeConfig> {
        BTreeMap::new()
    }

    #[test]
    fn test_mpsd_channel_bit_manipulation() {
        // (channel >> 1) & 0xFFF8 | (channel & 0x7)
        let cases = [
            (0x185, 197), // 389: (389 >> 1) & 0xFFF8 | (389 & 0x7) = 192 | 5 = 197
            (128, 64),    // 0b1000_0000: (128 >> 1) & 0xFFF8 | (128 & 0x7) = 64 | 0 = 64
        ];
        for (channel, expected_x) in cases {
            let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::neutron(100, channel);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].histo.x, expected_x, "channel={channel:#x}");
        }
    }

    #[test]
    fn test_mpsd_amplitude_to_y() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = Event::new(EventType::Neutron).with_raw(1234, 0);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.y, 1234);
    }

    #[test]
    fn test_mpsd_edge_mapping() {
        for (up, expected) in [(true, EventType::Tzero), (false, EventType::Edge { up: false })] {
            let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::edge(100, 5, up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, expected, "up={up}");
        }
    }

    #[test]
    fn test_mdll_neutron_sets_zero() {
        let mut recipe = Mdll::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let mut ev = test_utils::neutron(100, 1);
        ev.histo.x = 42;
        ev.histo.y = 99;
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 0);
        assert_eq!(out[0].histo.y, 0);
    }

    #[test]
    fn test_mdll_edge_mapping() {
        for (up, expected) in [(true, EventType::Tzero), (false, EventType::Edge { up: false })] {
            let mut recipe = Mdll::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::edge(100, 3, up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, expected, "up={up}");
        }
    }
}
