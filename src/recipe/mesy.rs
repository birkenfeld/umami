// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::expr::ExprAlias;
use crate::params::HasParams;
use super::Recipe;

// TODO: amplitude modes

/// What a rising edge on a given channel should be reinterpreted as.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeMapping {
    Tzero,
    Monitor(u8),
    Aux(u8),
}

impl EdgeMapping {
    fn to_evtype(self) -> (EventType, u8) {
        match self {
            EdgeMapping::Tzero => (EventType::Tzero, 0),
            EdgeMapping::Monitor(num) => (EventType::Monitor, num),
            EdgeMapping::Aux(num) => (EventType::AuxSignal, num)
        }
    }
}

#[derive(Debug, Deserialize, Clone, HasParams)]
#[params(kind = "recipe", type = "mesy_mpsd")]
pub struct Mpsd {
    /// Maps Edge channel to event type mapping; a channel not listed here
    /// will stay as Edge.
    #[serde(default, deserialize_with = "crate::util::deserialize_map_with_key")]
    #[param(help = "Maps interesting MCPD digital input channels to event type",
            datatype = "{{\"number\" = (\"tzero\"/\"monitor=num\"/\"aux=num\")}}")]
    inputs: BTreeMap<u32, EdgeMapping>,
    /// Voids out neutron events whose raw amplitude is 0 or > 960, a known
    /// bad/overflow region on this detector's amplitude channel.
    #[serde(default)]
    #[param(help = "Void out neutron events with raw amplitude == 0 or > 960")]
    y_mask: bool,
}

impl Recipe for Mpsd {
    fn from_config(cfg: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(cfg.try_into().context("Configuring MPSD recipe")?)
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    let x_orig = event.channel.0;
                    // since the MPSD has only 8 channels per board,
                    // remove fourth bit of channel
                    let x = (x_orig >> 1) & 0xFFF8 | (x_orig & 0x7);
                    event.histo.x = x as u16;

                    let y = event.raw[0] as u16;
                    if self.y_mask && (y == 0 || y > 960) {
                        event.evtype = EventType::Void;
                    }

                    event.histo.y = y; // TODO: calibration
                }
                EventType::Edge if event.index > 0 => {
                    if let Some(mapped) = self.inputs.get(&event.channel.0) {
                        let (evt, ix) = mapped.to_evtype();
                        event.evtype = evt;
                        event.index = ix;
                    }
                }
                _ => ()
            }
        }
        events
    }

    fn expr_aliases(&self) -> Vec<ExprAlias> {
        vec![
            ExprAlias::new("mesy_slot", "channel[0..4]", "MCPD slot in module"),
            ExprAlias::new("mesy_mod", "channel[4..7]", "MCPD module"),
            ExprAlias::new("mesy_mcpd", "channel[7..15]", "MCPD id"),
        ]
    }
}

#[derive(Debug, Deserialize, Clone, HasParams)]
#[params(kind = "recipe", type = "mesy_mdll")]
pub struct Mdll {}

impl Recipe for Mdll {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(Self {})
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Neutron => {
                    event.histo.x = 0; // TODO
                    event.histo.y = 0;
                }
                EventType::Edge if event.index > 0 => {
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

    #[test]
    fn test_mpsd_channel_bit_manipulation() {
        // (channel >> 1) & 0xFFF8 | (channel & 0x7)
        let cases = [
            (0x185, 197), // 389: (389 >> 1) & 0xFFF8 | (389 & 0x7) = 192 | 5 = 197
            (128, 64),    // 0b1000_0000: (128 >> 1) & 0xFFF8 | (128 & 0x7) = 64 | 0 = 64
        ];
        for (channel, expected_x) in cases {
            let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::neutron(100, channel);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].histo.x, expected_x, "channel={channel:#x}");
        }
    }

    #[test]
    fn test_mpsd_expr_aliases_match_channel_bit_layout() {
        let recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let mut table = crate::expr::AliasTable::new();
        for a in recipe.expr_aliases() {
            table.insert(a.name.clone(), a);
        }
        // channel = mcpd[7..15] | mod[4..7] | slot[0..4]
        let (mcpd, module, slot) = (5u32, 3u32, 9u32);
        let channel = (mcpd << 7) | (module << 4) | slot;
        let ev = test_utils::neutron(100, channel);
        let eval = |name| crate::expr::Expr::parse_with_aliases(name, &table).unwrap().eval(&ev);
        assert_eq!(eval("mesy_slot"), slot as i64);
        assert_eq!(eval("mesy_mod"), module as i64);
        assert_eq!(eval("mesy_mcpd"), mcpd as i64);
    }

    #[test]
    fn test_mpsd_amplitude_to_y() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = Event::new(EventType::Neutron).with_raw(1234, 0);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.y, 1234);
    }

    #[test]
    fn test_mpsd_y_mask_disabled_by_default() {
        let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let ev = Event::new(EventType::Neutron).with_raw(0, 0);
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Neutron);
    }

    #[test]
    fn test_mpsd_y_mask_voids_zero_and_overflow() {
        let cfg: toml::Table = toml::from_str("y_mask = true").unwrap();
        let cases = [(0, true), (960, false), (961, true), (500, false)];
        for (amp, expect_void) in cases {
            let mut recipe = Mpsd::from_config(cfg.clone(), &empty_recipes()).unwrap();
            let ev = Event::new(EventType::Neutron).with_raw(amp, 0);
            let out = recipe.process(vec![ev]);
            let expected = if expect_void { EventType::Void } else { EventType::Neutron };
            assert_eq!(out[0].evtype, expected, "amp={amp}");
        }
    }

    #[test]
    fn test_mpsd_unmapped_input_is_left_alone() {
        for up in [true, false] {
            let mut recipe = Mpsd::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::edge(100, 5, up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, EventType::Edge, "up={up}");
        }
    }

    #[test]
    fn test_mpsd_input_mapping() {
        let cfg: toml::Table = toml::from_str(r#"
            [inputs]
            3 = "tzero"
            5 = { monitor = 1 }
            7 = { aux = 2 }
        "#).unwrap();
        let mut recipe = Mpsd::from_config(cfg, &empty_recipes()).unwrap();

        let events = vec![
            test_utils::edge(100, 3, true),
            test_utils::edge(200, 5, true),
            test_utils::edge(300, 7, true),
            // down-edges and unmapped channels are untouched
            test_utils::edge(400, 3, false),
            test_utils::edge(500, 9, true),
        ];
        let out = recipe.process(events);
        assert_eq!(out[0].evtype, EventType::Tzero);
        assert_eq!(out[1].evtype, EventType::Monitor);
        assert_eq!(out[2].evtype, EventType::AuxSignal);
        assert_eq!(out[3].evtype, EventType::Edge);
        assert_eq!(out[4].evtype, EventType::Edge);
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
    fn test_mdll_edge_mapping() {
        for (up, expected) in [(true, EventType::Tzero), (false, EventType::Edge)] {
            let mut recipe = Mdll::from_config(toml::Table::new(), &empty_recipes()).unwrap();
            let ev = test_utils::edge(100, 3, up);
            let out = recipe.process(vec![ev]);
            assert_eq!(out[0].evtype, expected, "up={up}");
        }
    }
}
