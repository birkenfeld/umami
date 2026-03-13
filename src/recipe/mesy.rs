// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventData};
use super::Recipe;

#[derive(Debug, Deserialize, Clone)]
pub struct MesyTest {
}

impl Recipe for MesyTest {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(Self {})
    }

    fn update_config(&mut self, _: toml::Table) -> UResult<()> {
        Ok(())
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::RawDigital { value1: y, value2: x, .. } => {
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
