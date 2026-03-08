// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::event::{Event, EventData};
use super::Recipe;

const TUBE_RESOLUTION:  u32 = 256;
const MIDDLE_TUBE_RES:  u32 = 206;
const SMALL_TUBE_RES:   u32 = 94;
const MIDDLE_TUBE_OFFS: u32 = (TUBE_RESOLUTION - MIDDLE_TUBE_RES) / 2;
const SMALL_TUBE_OFFS:  u32 = (TUBE_RESOLUTION - SMALL_TUBE_RES) / 2;
const PIXEL_PER_PACK:   u32 = 8192;

const EXT_START: u32 = 1;

#[derive(Debug, Deserialize, Clone)]
pub struct KWSGERecipe {
    #[serde(default)]
    reso_1024: bool,
    #[serde(default)]
    rebin_8x8: bool,
    #[serde(default)]
    invert_ts: bool,
}

impl Recipe for KWSGERecipe {
    type Config = Self;

    fn from_config(config: Self::Config, _: &BTreeMap<String, RecipeConfig>) -> Self {
        config
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::RawNeutron => {
                    let mut x;
                    let mut y;
                    let mut id = event.input.0;
                    let n8p = id / PIXEL_PER_PACK;
                    id %= PIXEL_PER_PACK;

                    if self.reso_1024 {
                        x = id / 1024;
                        let y1 = id % 1024;
                        if !(3..=14).contains(&n8p) {
                            y = (y1 * SMALL_TUBE_RES) / 1024 + SMALL_TUBE_OFFS;
                        } else if !(6..=11).contains(&n8p) {
                            y = (y1 * MIDDLE_TUBE_RES) / 1024 + MIDDLE_TUBE_OFFS;
                        } else {
                            y = (y1 * TUBE_RESOLUTION) / 1024;
                        }
                    } else {
                        if !(3..=14).contains(&n8p) {
                            x = id / SMALL_TUBE_RES;
                            y = id % SMALL_TUBE_RES + SMALL_TUBE_OFFS;
                        } else if !(6..=11).contains(&n8p) {
                            x = id / MIDDLE_TUBE_RES;
                            y = id % MIDDLE_TUBE_RES + MIDDLE_TUBE_OFFS;
                        } else {
                            x = id / TUBE_RESOLUTION;
                            y = id % TUBE_RESOLUTION;
                        }
                    }
                    x += 8*n8p;
                    if self.rebin_8x8 {
                        y = 64 + y/2;
                    }

                    event.data = EventData::Neutron { x, y, t: 0 };
                }
                EventData::RawEdge { up } => {
                    match event.input.0 {
                        0 =>
                            event.data = EventData::Gate { up: up ^ self.invert_ts },
                        1 if up ^ self.invert_ts =>
                            event.data = EventData::Tzero,
                        3 if up ^ self.invert_ts =>
                            event.data = EventData::Tzero,  // TODO why?
                        2 if up ^ self.invert_ts =>
                            event.data = EventData::AuxSignal { value: EXT_START, up: true },
                        _ => ()
                    }
                }
                _ => ()
            }
        }
        events
    }
}
