// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::expr::ExprAlias;
use crate::params::HasParams;
use super::Recipe;

/// Computes the Jumiom PSD's X/Y position for Neutron events, bit-for-bit
/// ported from the `positionMode` switch in
/// `Jumiom/LibHelper/jumiom_data_helper.c`'s `jumpsd_fillhisto`. The FPGA
/// X/Y is read from `Event.channel` (X in the low byte, Y in the next
/// byte, as packed by the `jumiom` input); the 4 signed 12-bit ADC values
/// needed by every other mode are recovered losslessly from `Event.raw`
/// (the packed word2/word3 the input already preserves for this purpose).
/// Per-pixel limit-table filtering is not implemented (deferred, see
/// AGENTS.md / project notes).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositionMode {
    /// Raw FPGA X/Y passthrough.
    #[default]
    Fpga,
    /// Linear ADC-ratio calculation.
    Linear,
    /// "Entzerrung" distortion correction (U.Ruecker/V.Pipich, Mar. 2015).
    Distortion,
    /// Older empirical distortion formula ("Formula #0", V. Pipich).
    Formula0,
    /// Linear H8500-PMT formula ("Formula #2", V. Pipich).
    Formula2,
}

/// Sign-extends the 12-bit field at `shift` in `word` to a full `i32`.
fn adc12bit(word: u32, shift: u32) -> i32 {
    (((word >> shift) & 0xFFF) as i32) << 20 >> 20
}

fn unpack_adc(raw: (u32, u32)) -> [i32; 4] {
    [adc12bit(raw.0, 0), adc12bit(raw.0, 16), adc12bit(raw.1, 0), adc12bit(raw.1, 16)]
}

#[derive(Debug, Deserialize, Clone, HasParams)]
#[serde(deny_unknown_fields)]
pub struct Position {
    #[serde(default)]
    mode: PositionMode,
    // Distortion (mode 2) and Formula2 (mode 4) parameters.
    #[serde(default)]
    offset_x: f64,
    #[serde(default)]
    offset_y: f64,
    #[serde(default = "default_factor")]
    factor_x: f64,
    #[serde(default = "default_factor")]
    factor_y: f64,
    // Distortion (mode 2) only.
    #[serde(default)]
    a: f64,
    #[serde(default)]
    b: f64,
    #[serde(default)]
    c: f64,
    /// Distortion (mode 2) radius cutoff; no cutoff applied if <= 0.
    #[serde(default)]
    cutoff: f64,
}

fn default_factor() -> f64 {
    1.0
}

impl Position {
    /// Mode 1: linear calculation directly from the ADC ratios.
    fn linear(adc: [i32; 4]) -> (i32, i32) {
        let x = (adc[0] as f64 * 256.0 / (adc[0] + adc[1]) as f64).abs().round() as i32;
        let y = (adc[2] as f64 * 256.0 / (adc[2] + adc[3]) as f64).abs().round() as i32;
        (x, y)
    }

    /// Mode 2: "Entzerrung" distortion correction.
    fn distortion(&self, adc: [i32; 4]) -> Option<(i32, i32)> {
        let mut rrx = (adc[0] - adc[1]) as f64 / (adc[0] + adc[1]) as f64;
        let mut rry = (adc[2] - adc[3]) as f64 / (adc[2] + adc[3]) as f64;
        rrx += self.offset_x / 127.5;
        rry += self.offset_y / 127.5;
        rrx *= self.factor_x;
        rry *= self.factor_y;
        let rsq = rrx * rrx + rry * rry;
        let modu = if rsq == 0.0 { 0.0 } else { 4.0 * rrx * rrx * rry * rry / rsq / rsq };
        let fac = 1.0 + self.a * modu + self.b * rsq + self.c * modu * rsq;
        rrx /= fac;
        rry /= fac;
        if self.cutoff > 0.0 && rrx * rrx + rry * rry > self.cutoff * self.cutoff / 127.5 / 127.5 {
            return None;
        }
        if rrx * rrx > 1.0 || rry * rry > 1.0 {
            return None;
        }
        Some((((rrx + 1.0) * 127.5) as i32, ((rry + 1.0) * 127.5) as i32))
    }

    /// Mode 3: older empirical "Formula #0".
    fn formula0(adc: [i32; 4]) -> Option<(i32, i32)> {
        let mut rrx = (((adc[0] - adc[1]) as f64 / (adc[0] + adc[1]) as f64) + 1.0) * 128.0
            - (127.5 - 0.75);
        let mut rry = (((adc[2] - adc[3]) as f64 / (adc[2] + adc[3]) as f64) + 1.0) * 128.0
            - (127.5 - 1.0);
        rry = 0.9 * rry * 256.0 / 239.0;
        rrx = 0.9 * rrx * 256.0 / 250.0;
        let rad = rrx * rrx + rry * rry;
        let modu = if rad == 0.0 {
            0.0
        } else {
            let t = rrx * rry / rad;
            4.0 * t * t
        };
        let f = |r: f64| (1.0 + 0.0245 * modu) - (1.7748e-5 - 0.4939e-5 * modu) * r;
        let mut fac = f(rad);
        let mut rad_new = rad / fac / fac;
        fac = f(rad_new);
        rad_new = rad / fac / fac;
        fac = f(rad_new);
        fac = 1.0 / fac;
        if rrx * rrx * fac * fac + rry * rry * fac * fac > 127.5 * 127.5 {
            None
        } else {
            Some(((rrx * fac + 127.5) as i32, (rry * fac + 127.5) as i32))
        }
    }

    /// Mode 4: linear H8500-PMT "Formula #2".
    fn formula2(&self, adc: [i32; 4]) -> (i32, i32) {
        let sum = (adc[0] + adc[1] + adc[2] + adc[3]) as f64;
        let xm = ((adc[0] + adc[1]) - (adc[2] + adc[3])) as f64 / sum;
        let ym = ((adc[0] + adc[3]) - (adc[2] + adc[1])) as f64 / sum;
        ((self.factor_x * xm + self.offset_x) as i32, (self.factor_y * ym + self.offset_y) as i32)
    }
}

impl Recipe for Position {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(config.try_into().context("Configuring Jumiom position recipe")?)
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            if let EventType::Neutron = event.evtype {
                let xfpga = (event.channel.0 & 0xFF) as i32;
                let yfpga = ((event.channel.0 >> 8) & 0xFF) as i32;
                let pos = match self.mode {
                    PositionMode::Fpga => Some((xfpga, yfpga)),
                    PositionMode::Linear => Some(Self::linear(unpack_adc(event.raw))),
                    PositionMode::Distortion => self.distortion(unpack_adc(event.raw)),
                    PositionMode::Formula0 => Self::formula0(unpack_adc(event.raw)),
                    PositionMode::Formula2 => Some(self.formula2(unpack_adc(event.raw))),
                };
                match pos {
                    Some((x, y)) if (0..=255).contains(&x) && (0..=255).contains(&y) => {
                        event.histo.x = x as u16;
                        event.histo.y = y as u16;
                    }
                    _ => event.evtype = EventType::Void,
                }
            }
        }
        events
    }

    fn expr_aliases(&self) -> Vec<ExprAlias> {
        vec![
            ExprAlias::new("adc0", "raw_0[0..12:signed]", "Jumiom ADC0 (signed 12-bit)"),
            ExprAlias::new("adc1", "raw_0[16..28:signed]", "Jumiom ADC1 (signed 12-bit)"),
            ExprAlias::new("adc2", "raw_1[0..12:signed]", "Jumiom ADC2 (signed 12-bit)"),
            ExprAlias::new("adc3", "raw_1[16..28:signed]", "Jumiom ADC3 (signed 12-bit)"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;

    fn recipe(config: toml::Table) -> Position {
        Position::from_config(config, &BTreeMap::new()).unwrap()
    }

    fn neutron_with_raw(channel: u32, adc: [i32; 4]) -> Event {
        let enc = |v: i32| -> u32 { (v as u32) & 0xFFF };
        let word2 = (enc(adc[1]) << 16) | enc(adc[0]);
        let word3 = (enc(adc[3]) << 16) | enc(adc[2]);
        Event::new(EventType::Neutron).with_channel(channel).with_raw(word2, word3)
    }

    #[test]
    fn test_expr_aliases_match_unpack_adc_layout() {
        let r = recipe(toml::Table::new());
        let aliases = r.expr_aliases();
        let names: Vec<_> = aliases.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["adc0", "adc1", "adc2", "adc3"]);

        let mut table = crate::expr::AliasTable::new();
        for a in &aliases {
            table.insert(a.name.clone(), a.clone());
        }
        let adc = [100, -50, 2000, -2000];
        let ev = neutron_with_raw(0, adc);
        for (i, name) in ["adc0", "adc1", "adc2", "adc3"].iter().enumerate() {
            let got = crate::expr::Expr::parse_with_aliases(name, &table).unwrap().eval(&ev);
            assert_eq!(got, adc[i] as i64, "{name} mismatch");
        }
    }

    #[test]
    fn test_fpga_mode_extracts_x_y_from_channel() {
        let mut r = recipe(toml::Table::new());
        let channel = (3u32 << 8) | 5u32; // y=3, x=5
        let out = r.process(vec![test_utils::neutron(100, channel)]);
        assert_eq!(out[0].histo.x, 5);
        assert_eq!(out[0].histo.y, 3);
    }

    #[test]
    fn test_position_leaves_other_event_types_unchanged() {
        let mut r = recipe(toml::Table::new());
        let out = r.process(vec![test_utils::tzero(50)]);
        assert_eq!(out[0].evtype, EventType::Tzero);
        assert_eq!(out[0].histo.x, 0);
        assert_eq!(out[0].histo.y, 0);
    }

    #[test]
    fn test_linear_mode() {
        let mut cfg = toml::Table::new();
        cfg.insert("mode".into(), "linear".into());
        let mut r = recipe(cfg);
        // adc0=128,adc1=128 -> x = round(|128*256/256|) = 128
        // adc2=192,adc3=64  -> y = round(|192*256/256|) = 192
        let ev = neutron_with_raw(0, [128, 128, 192, 64]);
        let out = r.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[0].histo.x, 128);
        assert_eq!(out[0].histo.y, 192);
    }

    #[test]
    fn test_distortion_mode_identity_params() {
        let mut cfg = toml::Table::new();
        cfg.insert("mode".into(), "distortion".into());
        let mut r = recipe(cfg);
        // a=b=c=0 (no distortion), offset=0, factor=1, cutoff=0 (disabled):
        // rrx = (192-64)/(192+64) = 0.5 -> x = (0.5+1)*127.5 = 191 (truncated)
        // rry = (64-192)/(64+192) = -0.5 -> y = (-0.5+1)*127.5 = 63 (truncated)
        let ev = neutron_with_raw(0, [192, 64, 64, 192]);
        let out = r.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[0].histo.x, 191);
        assert_eq!(out[0].histo.y, 63);
    }

    #[test]
    fn test_distortion_mode_rejects_out_of_cutoff() {
        let mut cfg = toml::Table::new();
        cfg.insert("mode".into(), "distortion".into());
        cfg.insert("cutoff".into(), 1.0.into());
        let mut r = recipe(cfg);
        // rrx=1, rry=0 (adc1=adc3=0): well outside a cutoff of 1/127.5
        let ev = neutron_with_raw(0, [100, 0, 1, 0]);
        let out = r.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Void);
    }

    #[test]
    fn test_formula2_mode() {
        let mut cfg = toml::Table::new();
        cfg.insert("mode".into(), "formula2".into());
        let mut r = recipe(cfg);
        // sum=200; xm=((100+100)-(0+0))/200=1.0; ym=((100+0)-(0+100))/200=0.0
        // x = factor_x(1.0)*xm + offset_x(0.0) = 1.0 -> 1
        // y = factor_y(1.0)*ym + offset_y(0.0) = 0.0 -> 0
        let ev = neutron_with_raw(0, [100, 100, 0, 0]);
        let out = r.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[0].histo.x, 1);
        assert_eq!(out[0].histo.y, 0);
    }

    #[test]
    fn test_formula0_mode_does_not_panic_and_stays_in_range_or_voids() {
        let mut cfg = toml::Table::new();
        cfg.insert("mode".into(), "formula0".into());
        let mut r = recipe(cfg);
        // Symmetric input: both ratios 0, so rrx/rry are just the fixed
        // centering offsets scaled -- mainly a smoke test for the formula
        // (iterative fac computation, division-heavy) not blowing up.
        let ev = neutron_with_raw(0, [128, 128, 128, 128]);
        let out = r.process(vec![ev]);
        assert!(matches!(out[0].evtype, EventType::Neutron | EventType::Void));
    }
}
