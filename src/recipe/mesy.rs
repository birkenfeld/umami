// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::event::{Event, EventData};
use super::Recipe;

#[derive(Debug, Deserialize, Clone)]
pub struct MesyTest {
}

impl Recipe for MesyTest {
    type Config = ();

    fn from_config(_config: Self::Config, _: &BTreeMap<String, RecipeConfig>) -> Self {
        Self {}
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::RawDigital { value1, .. } => {
                    event.data = EventData::Neutron { x: event.input.0, y: value1, t: 0 };
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
