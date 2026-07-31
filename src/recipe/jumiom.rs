// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::PathBuf;
use anyhow::{anyhow, Context};
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
///
/// A per-pixel accept-window gate (`jumpsd_fillhisto`'s limit table) always
/// applies too, `limits_file` or not -- see [`Position::passes_limit_table`].
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

/// The "adchelper" value used to index the limit table: each ADC value
/// (shifted down 3 more bits) is summed only when non-negative, then the
/// sum is divided by 4 regardless of how many terms were actually added.
fn adchelper(adc: [i32; 4]) -> i32 {
    let sum: i32 = adc.iter().map(|v| v >> 3).filter(|&v| v >= 0).sum();
    sum >> 2
}

const LIMIT_TABLE_SIZE: usize = 256 * 256;

#[derive(Debug, Deserialize, Clone, HasParams)]
#[serde(deny_unknown_fields)]
#[params(kind = "recipe", type = "jumiom")]
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
    /// Per-pixel accept-window ("limits") file: lines of `_ _ lower upper _`
    /// (5 whitespace-separated ints; only the middle two are used), exactly
    /// 65536 lines, ordered y-major (all x for y=0, then y=1, ...). If unset,
    /// no limit-table filtering is applied at all.
    #[serde(default)]
    limits_file: Option<PathBuf>,
    /// Use the FPGA-reported (not the calculated) position to index the
    /// limit table. Only meaningful together with `limits_file`.
    #[serde(default)]
    use_fpga_for_limit_index: bool,
    #[serde(skip)]
    lower_limit: Vec<u32>,
    #[serde(skip)]
    upper_limit: Vec<u32>,
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

    /// Loads `lower_limit`/`upper_limit` from `limits_file`, or fills them
    /// with an accept-everything default (`[0, 255]`) if unset.
    fn load_limits_table(&mut self) -> UResult<()> {
        let Some(path) = &self.limits_file else {
            self.lower_limit = vec![0; LIMIT_TABLE_SIZE];
            self.upper_limit = vec![255; LIMIT_TABLE_SIZE];
            return Ok(());
        };
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Reading Jumiom limits file {path:?}"))?;
        let mut lower = Vec::with_capacity(LIMIT_TABLE_SIZE);
        let mut upper = Vec::with_capacity(LIMIT_TABLE_SIZE);
        for (lineno, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [_, _, ll, ul, _] = fields[..] else {
                return Err(anyhow!(
                    "Jumiom limits file {path:?}: expected 5 fields on line {}, found {}",
                    lineno + 1, fields.len()).into());
            };
            let parse = |s: &str, what: &str| -> UResult<u32> {
                Ok(s.parse().with_context(|| format!(
                    "Jumiom limits file {path:?}: bad {what} on line {}", lineno + 1))?)
            };
            lower.push(parse(ll, "lower limit")?);
            upper.push(parse(ul, "upper limit")?);
        }
        if lower.len() != LIMIT_TABLE_SIZE {
            return Err(anyhow!(
                "Jumiom limits file {path:?}: expected {LIMIT_TABLE_SIZE} entries, found {}",
                lower.len()).into());
        }
        self.lower_limit = lower;
        self.upper_limit = upper;
        Ok(())
    }

    /// Per-pixel accept-window gate from `jumpsd_fillhisto`, applied after
    /// the position has already been computed and bounds-checked. Always
    /// active, `limits_file` or not: with no file, `lower_limit`/
    /// `upper_limit` default to accept-everything, but the `adchelper`/
    /// `ymerk` guards below still apply unconditionally.
    fn passes_limit_table(&self, xfpga: i32, yfpga: i32, xmerk: i32, ymerk: i32,
                          raw: (u32, u32)) -> bool {
        let helper = adchelper(unpack_adc(raw));
        if !(helper > 0 && helper < 256 && ymerk > 0) {
            return false;
        }
        let (iy, ix) = if self.use_fpga_for_limit_index {
            (yfpga, xfpga)
        } else {
            (ymerk, xmerk)
        };
        let idx = iy as usize * 256 + ix as usize;
        let helper = helper as u32;
        helper >= self.lower_limit[idx] && helper <= self.upper_limit[idx]
    }
}

impl Recipe for Position {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let mut this: Self = config.try_into().context("Configuring Jumiom position recipe")?;
        this.load_limits_table()?;
        Ok(this)
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
                    Some((x, y)) if (0..=255).contains(&x) && (0..=255).contains(&y)
                        && self.passes_limit_table(xfpga, yfpga, x, y, event.raw) =>
                    {
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

    /// Writes a full 256x256 limits file (accept-all `[0, 255]` everywhere
    /// except `overrides`, each an `(x, y, lower, upper)` tuple) to a fresh
    /// temp file and returns its path.
    fn write_limits_file(overrides: &[(usize, usize, u32, u32)]) -> std::path::PathBuf {
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir()
            .join(format!("umami_test_jumiom_limits_{}_{n}.tab", std::process::id()));
        let mut table = vec![(0u32, 255u32); LIMIT_TABLE_SIZE];
        for &(x, y, lo, hi) in overrides {
            table[y * 256 + x] = (lo, hi);
        }
        let mut file = std::fs::File::create(&path).unwrap();
        for (lo, hi) in table {
            writeln!(file, "0 0 {lo} {hi} 0").unwrap();
        }
        path
    }

    #[test]
    fn test_adchelper_computation() {
        // adcval[i] = adc12bit[i] >> 3; sum only non-negative terms; result >> 2
        // adc=[100,-50,2000,-2000] -> shifted: [12,-7,250,-250]; only 12+250=262
        // is summed (negatives dropped); 262 >> 2 = 65
        assert_eq!(adchelper([100, -50, 2000, -2000]), 65);
        // all negative -> sum stays 0
        assert_eq!(adchelper([-1, -2, -3, -4]), 0);
    }

    #[test]
    fn test_no_limits_file_still_rejects_y_zero_and_bad_adchelper() {
        // the ymerk>0 and adchelper guards are unconditional in the C
        // source -- they apply even with no limits_file configured, only
        // the per-pixel window itself defaults to accept-everything
        let mut r = recipe(toml::Table::new());
        let channel = 5u32; // y=0, x=5 (Fpga mode default)
        let out = r.process(vec![neutron_with_raw(channel, [100, 100, 100, 100])]);
        assert_eq!(out[0].evtype, EventType::Void, "y=0 must always be rejected");

        let channel = (3u32 << 8) | 5u32; // y=3, x=5
        let out = r.process(vec![test_utils::neutron(100, channel)]); // raw=(0,0) -> adchelper=0
        assert_eq!(out[0].evtype, EventType::Void, "adchelper=0 must always be rejected");
    }

    #[test]
    fn test_no_limits_file_accepts_valid_position_and_adchelper() {
        let mut r = recipe(toml::Table::new());
        let channel = (3u32 << 8) | 5u32; // y=3, x=5
        let out = r.process(vec![neutron_with_raw(channel, [100, 100, 100, 100])]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[0].histo.x, 5);
        assert_eq!(out[0].histo.y, 3);
    }

    #[test]
    fn test_limits_file_rejects_outside_window() {
        let path = write_limits_file(&[(5, 3, 10, 20)]);
        let mut cfg = toml::Table::new();
        cfg.insert("limits_file".into(), path.to_string_lossy().into_owned().into());
        let mut r = recipe(cfg);

        // Fpga mode: x=5, y=3
        let channel = (3u32 << 8) | 5u32;
        // adchelper = (100>>3)*4 >> 2 = 12, inside [10,20]
        let out = r.process(vec![neutron_with_raw(channel, [100, 100, 100, 100])]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!((out[0].histo.x, out[0].histo.y), (5, 3));

        // adchelper = (2000>>3)*4 >> 2 = 250, outside [10,20]
        let out = r.process(vec![neutron_with_raw(channel, [2000, 2000, 2000, 2000])]);
        assert_eq!(out[0].evtype, EventType::Void);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_limits_file_use_fpga_index_toggle() {
        // linear mode's calculated position (128, 192) differs from the
        // FPGA-encoded one (9, 9); close the window at the calculated
        // pixel and leave the FPGA pixel at the default accept-all, so the
        // toggle alone decides whether this event is accepted
        let path = write_limits_file(&[(128, 192, 255, 0)]); // impossible window
        let channel = (9u32 << 8) | 9u32; // FPGA y=9, x=9
        let adc = [128, 128, 192, 64]; // linear mode -> calculated (128, 192)

        let mut cfg_fpga = toml::Table::new();
        cfg_fpga.insert("mode".into(), "linear".into());
        cfg_fpga.insert("limits_file".into(), path.to_string_lossy().into_owned().into());
        cfg_fpga.insert("use_fpga_for_limit_index".into(), true.into());
        let mut r_fpga = recipe(cfg_fpga);
        let out = r_fpga.process(vec![neutron_with_raw(channel, adc)]);
        assert_eq!(out[0].evtype, EventType::Neutron, "FPGA-index lookup should accept");

        let mut cfg_calc = toml::Table::new();
        cfg_calc.insert("mode".into(), "linear".into());
        cfg_calc.insert("limits_file".into(), path.to_string_lossy().into_owned().into());
        let mut r_calc = recipe(cfg_calc);
        let out = r_calc.process(vec![neutron_with_raw(channel, adc)]);
        assert_eq!(out[0].evtype, EventType::Void, "calculated-index lookup should reject");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_limits_file_ymerk_zero_is_always_rejected() {
        let path = write_limits_file(&[]); // wide open everywhere
        let mut cfg = toml::Table::new();
        cfg.insert("limits_file".into(), path.to_string_lossy().into_owned().into());
        let mut r = recipe(cfg);

        let channel = 5u32; // y=0, x=5
        let out = r.process(vec![neutron_with_raw(channel, [100, 100, 100, 100])]);
        assert_eq!(out[0].evtype, EventType::Void);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_limits_file_wrong_line_count_errors() {
        use std::io::Write;
        let path = std::env::temp_dir()
            .join(format!("umami_test_jumiom_bad_{}.tab", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "0 0 0 255 0").unwrap(); // only 1 line, not 65536
        drop(file);

        let mut cfg = toml::Table::new();
        cfg.insert("limits_file".into(), path.to_string_lossy().into_owned().into());
        let result = Position::from_config(cfg, &BTreeMap::new());
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
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
        // non-zero ADCs so adchelper clears the always-on limit-table gate
        let out = r.process(vec![neutron_with_raw(channel, [100, 100, 100, 100])]);
        assert_eq!(out[0].evtype, EventType::Neutron);
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
        // sum=200; xm=((100+0)-(0+100))/200=0.0; ym=((100+100)-(0+0))/200=1.0
        // x = factor_x(1.0)*xm + offset_x(0.0) = 0.0 -> 0
        // y = factor_y(1.0)*ym + offset_y(0.0) = 1.0 -> 1
        // (y=1, not 0, so this doesn't trip the always-on ymerk>0 gate)
        let ev = neutron_with_raw(0, [100, 0, 0, 100]);
        let out = r.process(vec![ev]);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[0].histo.x, 0);
        assert_eq!(out[0].histo.y, 1);
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
