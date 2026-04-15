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
                EventData::RawDigital { value1: pl, value2: pr, .. } => {
                    // 8 tubes per module
                    let x = event.channel.0;
                    // TODO: calibration
                    let y = ((f64::from(pr) / f64::from(pr + pl)) * self.reso as f64) as u32;
                    event.data = EventData::Neutron { x, y, t: 0 };
                }
                EventData::RawEdge { .. } => {
                    event.data = EventData::Tzero;
                }
                _ => ()
            }
        }
        events
    }
}
