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

    #[test]
    fn test_kws_neutron_full_tube_region() {
        // n8p in 6..=11 → full tube resolution (256)
        // pack 6, pixel 0 → id=0, x = 0/256 + 8*6 = 48, y = 0%256 = 0
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 6 * PIXEL_PER_PACK);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 48);
        assert_eq!(out[0].histo.y, 0);
    }

    #[test]
    fn test_kws_neutron_full_tube_nonzero() {
        // pack 6, pixel 256 → id=256, x = 256/256 + 48 = 49, y = 256%256 = 0
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 6 * PIXEL_PER_PACK + 256);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 49);
        assert_eq!(out[0].histo.y, 0);
    }

    #[test]
    fn test_kws_neutron_small_tube_region() {
        // n8p=2 (not in 3..=14) → small tube resolution (94), offset 81
        // pack 2, pixel 0 → id=0, x = 0/94 + 8*2 = 16, y = 0%94 + 81 = 81
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 2 * PIXEL_PER_PACK);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 16);
        assert_eq!(out[0].histo.y, 81);
    }

    #[test]
    fn test_kws_neutron_middle_tube_region() {
        // n8p=4 (in 3..=14 but not in 6..=11) → middle tube resolution (206), offset 25
        // pack 4, pixel 0 → id=0, x = 0/206 + 8*4 = 32, y = 0%206 + 25 = 25
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 4 * PIXEL_PER_PACK);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 32);
        assert_eq!(out[0].histo.y, 25);
    }

    #[test]
    fn test_kws_neutron_reso_1024() {
        // n8p=6 (full tube, uses 1024-domain scaling instead of 256-domain)
        // pack 6, pixel 512 → x = 512/1024 + 8*6 = 48, y = 512*256/1024 = 128
        let mut cfg = toml::Table::new();
        cfg.insert("reso_1024".into(), toml::Value::Boolean(true));
        let mut recipe = KWSGERecipe::from_config(cfg, &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 6 * PIXEL_PER_PACK + 512);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 48);
        assert_eq!(out[0].histo.y, 128);
    }

    #[test]
    fn test_kws_neutron_rebin_8x8() {
        // pack 6, pixel 100 → x = 100/256 + 48 = 48, y = 100%256 = 100
        // rebinned: y = 64 + 100/2 = 114
        let mut cfg = toml::Table::new();
        cfg.insert("rebin_8x8".into(), toml::Value::Boolean(true));
        let mut recipe = KWSGERecipe::from_config(cfg, &empty_recipes()).unwrap();
        let ev = test_utils::neutron(100, 6 * PIXEL_PER_PACK + 100);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 48);
        assert_eq!(out[0].histo.y, 114);
    }

    #[test]
    fn test_kws_edge_to_tzero_ch3() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 3, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Tzero);
    }

    #[test]
    fn test_kws_edge_unmatched_channel_unchanged() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 5, true);
        let out = recipe.process(vec![ev]);
        assert!(matches!(out[0].evtype, EventType::Edge { up: true }));
    }

    #[test]
    fn test_kws_edge_to_gate() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 0, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Gate { up: true });
    }

    #[test]
    fn test_kws_edge_to_tzero_ch1() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 1, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Tzero);
    }

    #[test]
    fn test_kws_edge_to_aux_ch2() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 2, true);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::AuxSignal { num: EXT_START });
    }

    #[test]
    fn test_kws_edge_no_match_down() {
        let mut recipe = KWSGERecipe::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = test_utils::edge(100, 1, false);
        let out = recipe.process(vec![ev]);
        // down edge on ch1 doesn't match `up ^ false` → stays Edge
        assert!(matches!(out[0].evtype, EventType::Edge { up: false }));
    }

    #[test]
    fn test_kws_invert_ts() {
        let mut cfg = toml::Table::new();
        cfg.insert("invert_ts".into(), toml::Value::Boolean(true));
        let mut recipe = KWSGERecipe::from_config(cfg, &empty_recipes()).unwrap();
        // down edge on ch1: up=false, invert=true → false^true=true → Tzero
        let ev = test_utils::edge(100, 1, false);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Tzero);
    }
}
