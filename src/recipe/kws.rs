// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::event::{Event, EventType};
use crate::error::UResult;
use crate::params::HasParams;
use super::Recipe;

const TUBE_RESOLUTION:  u32 = 256;
const MIDDLE_TUBE_RES:  u32 = 206;
const SMALL_TUBE_RES:   u32 = 94;
const MIDDLE_TUBE_OFFS: u32 = (TUBE_RESOLUTION - MIDDLE_TUBE_RES) / 2;
const SMALL_TUBE_OFFS:  u32 = (TUBE_RESOLUTION - SMALL_TUBE_RES) / 2;
const PIXEL_PER_PACK:   u32 = 8192;

const EXT_START: u8 = 1;

#[derive(Debug, Deserialize, Clone, HasParams)]
#[serde(deny_unknown_fields)]
pub struct KWSGERecipe {
    #[serde(default)]
    reso_1024: bool,
    #[serde(default)]
    rebin_8x8: bool,
    #[serde(default)]
    invert_ts: bool,
}

impl Recipe for KWSGERecipe {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let this = config.try_into()
            .context("parsing config for tof_std recipe")?;
        Ok(this)
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    let mut x;
                    let mut y;
                    let mut id = event.channel.0;
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

                    event.histo.x = x as u16;
                    event.histo.y = y as u16;
                }
                EventType::Edge { up } => {
                    match event.channel.0 {
                        0 =>
                            event.evtype = EventType::Gate { up: up ^ self.invert_ts },
                        1 if up ^ self.invert_ts =>
                            event.evtype = EventType::Tzero,
                        3 if up ^ self.invert_ts =>
                            event.evtype = EventType::Tzero,
                        2 if up ^ self.invert_ts =>
                            event.evtype = EventType::AuxSignal { num: EXT_START },
                        _ => ()
                    }
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

    struct NeutronCase {
        reso_1024: bool,
        rebin_8x8: bool,
        channel: u32,
        x: u16,
        y: u16,
        desc: &'static str,
    }

    #[test]
    fn test_kws_neutron_binning() {
        let cases = [
            // n8p in 6..=11 -> full tube resolution (256)
            // pack 6, pixel 0 -> id=0, x = 0/256 + 8*6 = 48, y = 0%256 = 0
            NeutronCase { reso_1024: false, rebin_8x8: false, channel: 6 * PIXEL_PER_PACK,
                          x: 48, y: 0, desc: "full tube region" },
            // pack 6, pixel 256 -> id=256, x = 256/256 + 48 = 49, y = 256%256 = 0
            NeutronCase { reso_1024: false, rebin_8x8: false, channel: 6 * PIXEL_PER_PACK + 256,
                          x: 49, y: 0, desc: "full tube, nonzero pixel" },
            // n8p=2 (not in 3..=14) -> small tube resolution (94), offset 81
            // pack 2, pixel 0 -> id=0, x = 0/94 + 8*2 = 16, y = 0%94 + 81 = 81
            NeutronCase { reso_1024: false, rebin_8x8: false, channel: 2 * PIXEL_PER_PACK,
                          x: 16, y: 81, desc: "small tube region" },
            // n8p=4 (in 3..=14 but not 6..=11) -> middle tube resolution (206), offset 25
            // pack 4, pixel 0 -> id=0, x = 0/206 + 8*4 = 32, y = 0%206 + 25 = 25
            NeutronCase { reso_1024: false, rebin_8x8: false, channel: 4 * PIXEL_PER_PACK,
                          x: 32, y: 25, desc: "middle tube region" },
            // n8p=6 (full tube, 1024-domain scaling instead of 256-domain)
            // pack 6, pixel 512 -> x = 512/1024 + 8*6 = 48, y = 512*256/1024 = 128
            NeutronCase { reso_1024: true, rebin_8x8: false, channel: 6 * PIXEL_PER_PACK + 512,
                          x: 48, y: 128, desc: "reso_1024 scaling" },
            // pack 6, pixel 100 -> x = 100/256 + 48 = 48, y = 100%256 = 100
            // rebinned: y = 64 + 100/2 = 114
            NeutronCase { reso_1024: false, rebin_8x8: true, channel: 6 * PIXEL_PER_PACK + 100,
                          x: 48, y: 114, desc: "rebin_8x8" },
        ];

        for case in cases {
            let mut cfg = toml::Table::new();
            cfg.insert("reso_1024".into(), toml::Value::Boolean(case.reso_1024));
            cfg.insert("rebin_8x8".into(), toml::Value::Boolean(case.rebin_8x8));
            let mut recipe = KWSGERecipe::from_config(cfg, &empty_recipes()).unwrap();
            let ev = test_utils::neutron(100, case.channel);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].histo.x, case.x, "{}: x", case.desc);
            assert_eq!(out[0].histo.y, case.y, "{}: y", case.desc);
        }
    }

    struct EdgeCase {
        invert_ts: bool,
        channel: u32,
        up: bool,
        expected: EventType,
        desc: &'static str,
    }

    #[test]
    fn test_kws_edge_mapping() {
        let cases = [
            EdgeCase { invert_ts: false, channel: 0, up: true,
                       expected: EventType::Gate { up: true }, desc: "ch0 -> gate" },
            EdgeCase { invert_ts: false, channel: 1, up: true,
                       expected: EventType::Tzero, desc: "ch1 up -> tzero" },
            EdgeCase { invert_ts: false, channel: 3, up: true,
                       expected: EventType::Tzero, desc: "ch3 up -> tzero" },
            EdgeCase { invert_ts: false, channel: 2, up: true,
                       expected: EventType::AuxSignal { num: EXT_START }, desc: "ch2 up -> aux" },
            EdgeCase { invert_ts: false, channel: 1, up: false,
                       expected: EventType::Edge { up: false },
                       desc: "ch1 down, no invert -> unchanged" },
            EdgeCase { invert_ts: true, channel: 1, up: false,
                       expected: EventType::Tzero, desc: "ch1 down with invert_ts -> tzero" },
            EdgeCase { invert_ts: false, channel: 5, up: true,
                       expected: EventType::Edge { up: true }, desc: "unmatched channel -> unchanged" },
        ];

        for case in cases {
            let mut cfg = toml::Table::new();
            cfg.insert("invert_ts".into(), toml::Value::Boolean(case.invert_ts));
            let mut recipe = KWSGERecipe::from_config(cfg, &empty_recipes()).unwrap();
            let ev = test_utils::edge(100, case.channel, case.up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, case.expected, "{}", case.desc);
        }
    }
}
