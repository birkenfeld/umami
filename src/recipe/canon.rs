// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::params::HasParams;
use super::Recipe;

#[derive(Debug, Deserialize, Clone, HasParams)]
pub struct Psd {
    reso: usize,
}

impl Recipe for Psd {
    fn from_config(cfg: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(cfg.try_into().context("Configuring Canon recipe")?)
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    // 8 tubes per module
                    let x = event.channel.0;
                    // TODO: calibration
                    let (pl, pr) = event.raw;
                    let y = (f64::from(pr) / f64::from(pr + pl)) * self.reso as f64;
                    event.histo.x = x as u16;
                    event.histo.y = y as u16;
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

    fn psd(reso: i64) -> Psd {
        let mut cfg = toml::Table::new();
        cfg.insert("reso".into(), toml::Value::Integer(reso));
        Psd::from_config(cfg, &empty_recipes()).unwrap()
    }

    #[test]
    fn test_psd_neutron_binning() {
        let mut recipe = psd(256);

        // x comes straight from the channel; y = pr / (pr + pl) * reso
        let ev = Event::new(EventType::Neutron).with_channel(42).with_raw(100, 300);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 42);
        assert_eq!(out[0].histo.y, 192); // 300 / 400 * 256

        let ev = Event::new(EventType::Neutron).with_channel(0).with_raw(50, 50);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.y, 128); // 50 / 100 * 256
    }

    #[test]
    fn test_psd_edge_mapping() {
        for (up, expected) in [(true, EventType::Tzero), (false, EventType::Edge { up: false })] {
            let mut recipe = psd(256);
            let ev = test_utils::edge(100, 5, up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, expected, "up={up}");
        }
    }
}
