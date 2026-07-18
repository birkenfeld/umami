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
                    event.x = x;
                    // event.y = y; TODO
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
                    event.x = 0; // TODO
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
