// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventData};
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
            match event.data {
                EventData::Neutron => {
                    // 8 tubes per module
                    let x = event.channel.0;
                    // TODO: calibration
                    // let y = ((f64::from(pr) / f64::from(pr + pl)) * self.reso as f64) as u32;
                    event.x = x;
                    event.y = 0;
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

    fn empty_recipes() -> BTreeMap<String, RecipeConfig> {
        BTreeMap::new()
    }

    #[test]
    fn test_psd_neutron_channel_to_x() {
        let mut cfg = toml::Table::new();
        cfg.insert("reso".into(), toml::Value::Integer(256));
        let mut recipe = Psd::from_config(cfg, &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 42);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].x, 42);
        assert_eq!(out[0].y, 0);
    }

    #[test]
    fn test_psd_edge_up_to_tzero() {
        let mut cfg = toml::Table::new();
        cfg.insert("reso".into(), toml::Value::Integer(256));
        let mut recipe = Psd::from_config(cfg, &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 5, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].data, EventData::Tzero);
    }

    #[test]
    fn test_psd_edge_down_unchanged() {
        let mut cfg = toml::Table::new();
        cfg.insert("reso".into(), toml::Value::Integer(256));
        let mut recipe = Psd::from_config(cfg, &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 5, false);
        let out = recipe.process(vec![ev]);
        assert!(matches!(out[0].data, EventData::Edge { up: false }));
    }
}
