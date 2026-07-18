// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventData};
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
            match event.data {
                EventData::Neutron => {
                    let x_orig = event.channel.0;
                    // since the MPSD has only 8 channels per board,
                    // remove fourth bit of channel
                    let x = (x_orig >> 1) & 0xFFF8 | (x_orig & 0x7);
                    event.histo.x = x as u16;
                    event.histo.y = event.raw.0 as u16; // TODO: calibration
                }
                EventData::Edge { up: true } => {
                    event.data = EventData::Tzero;
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
            match event.data {
                EventData::Neutron => {
                    event.histo.x = 0; // TODO
                    event.histo.y = 0;
                }
                EventData::Edge { up: true } => {
                    event.data = EventData::Tzero;
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
    use crate::event::ChannelId;

    fn empty_recipes() -> BTreeMap<String, RecipeConfig> {
        BTreeMap::new()
    }

    #[test]
    fn test_mpsd_channel_bit_manipulation() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        // channel 0x185 = 389
        // (389 >> 1) & 0xFFF8 | (389 & 0x7) = 192 | 5 = 197
        let mut ev = test_utils::neutron(100, 0x185);
        ev.channel = ChannelId(0x185);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 197);
    }

    #[test]
    fn test_mpsd_simple_channel() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        // channel 0b1000_0000 = 128
        // (128 >> 1) & 0xFFF8 | (128 & 0x7)
        // = 64 & 0xFFF8 | 0
        // = 0x40 | 0 = 64
        let mut ev = test_utils::neutron(100, 128);
        ev.channel = ChannelId(128);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 64);
    }

    #[test]
    fn test_mpsd_edge_up_to_tzero() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 5, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].data, EventData::Tzero);
    }

    #[test]
    fn test_mpsd_edge_down_unchanged() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 5, false);
        let out = recipe.process(vec![ev]);
        assert!(matches!(out[0].data, EventData::Edge { up: false }));
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
    fn test_mdll_edge_up_to_tzero() {
        let mut recipe = Mdll::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 3, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].data, EventData::Tzero);
    }

    #[test]
    fn test_mdll_edge_down_unchanged() {
        let mut recipe = Mdll::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 3, false);
        let out = recipe.process(vec![ev]);
        assert!(matches!(out[0].data, EventData::Edge { up: false }));
    }
}
