// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! User-defined auxiliary/diagnostic histograms: each configured histogram
//! gets its own POSIX shm segment, filled by evaluating a filter + one or
//! two axis expressions against every event. The whole histogram list is
//! runtime-replaceable via `SetParams`: setting unlinks all current shm
//! segments and recreates fresh ones. Segments are not unlinked on drop,
//! matching the main histogram's own lifecycle (they persist in /dev/shm
//! across a brief restart).
//!
//! A separate `enabled` param (default true) is a global on/off switch:
//! when off, `handle_events` skips all filter/axis evaluation entirely.

use std::collections::HashSet;
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use crate::command::ModuleId;
use crate::config::HistoConfig;
use crate::error::UResult;
use crate::event::{Event, EventHisto};
use crate::expr::Expr;
use crate::params::HasParams;
use crate::shm::{ShmBox, ShmInterface};
use super::{Output, OutputCommon};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisSpec {
    pub expr: String,
    pub bins: u16,
    /// Inclusive lower bound of the binned range.
    pub min: i64,
    /// Inclusive upper bound of the binned range (unlike e.g. Rust's `..`
    /// ranges, `max` itself is a valid, in-range value).
    pub max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoSpec {
    pub name: String,
    #[serde(default)]
    pub filter: Option<String>,
    pub x: AxisSpec,
    #[serde(default)]
    pub y: Option<AxisSpec>,
}

#[derive(Debug, Deserialize)]
struct AuxHistoConfig {
    #[serde(default)]
    histos: Vec<HistoSpec>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// A compiled, ready-to-evaluate axis: everything `handle_events` needs
struct CompiledAxis {
    expr: Expr,
    bins: u16,
    min: i64,
    max: i64,
}

fn compile_axis(axis: &AxisSpec, histo_name: &str) -> UResult<CompiledAxis> {
    if axis.bins == 0 || axis.max < axis.min {
        return Err(anyhow!(
            "Invalid axis range for histogram {histo_name:?} (need bins > 0, max >= min)").into());
    }
    let expr = Expr::parse(&axis.expr)
        .with_context(|| format!("Parsing expression for histogram {histo_name:?}"))?;
    Ok(CompiledAxis { expr, bins: axis.bins, min: axis.min, max: axis.max })
}

/// Fully self-contained runtime state for one histogram
struct CompiledHisto {
    filter: Expr,
    x: CompiledAxis,
    y: Option<CompiledAxis>,
    shm_name: String,
    shm: ShmBox,
}

/// Validated but not-yet-materialized histogram
struct PreparedHisto {
    name: String,
    filter: Expr,
    x: CompiledAxis,
    y: Option<CompiledAxis>,
    shm_name: String,
    histo_config: HistoConfig,
}

/// `max` is inclusive, so the range spans `max - min + 1` representable
/// values (e.g. `bins=56, min=0, max=55` puts each of the 56 integer values
/// 0..=55 in its own bin).
fn bin_index(v: i64, min: i64, max: i64, bins: u16) -> Option<u16> {
    if v < min || v > max || bins == 0 {
        return None;
    }
    let bin = (v - min) * bins as i64 / (max - min + 1);
    Some(bin.clamp(0, bins as i64 - 1) as u16)
}

#[derive(HasParams)]
pub struct AuxHistoOutput {
    ipc_name: String,
    name: ModuleId,
    #[param(help = "Global on/off switch")]
    enabled: bool,
    #[param(help = "List of auxiliary histogram definitions",
            has_setter = true, datatype = "array of histogram specs")]
    histos: Vec<HistoSpec>,
    compiled: Vec<CompiledHisto>,
}

impl AuxHistoOutput {
    fn shm_name(&self, histo_name: &str) -> String {
        format!("{}_{}_{}", self.ipc_name, self.name, histo_name)
    }

    /// Parses and validates a spec list (expressions, axis ranges, name
    /// uniqueness) without creating or touching any shm segment.
    fn prepare(&self, specs: &[HistoSpec]) -> UResult<Vec<PreparedHisto>> {
        let mut seen_names = HashSet::new();
        let mut result = Vec::with_capacity(specs.len());
        for spec in specs {
            if !seen_names.insert(spec.name.clone()) {
                return Err(anyhow!("Duplicate histogram name {:?}", spec.name).into());
            }
            let filter = match &spec.filter {
                Some(f) => Expr::parse(f)
                    .with_context(|| format!("Parsing filter for histogram {:?}", spec.name))?,
                None => Expr::parse("1").expect("constant '1' always parses"),
            };
            let x = compile_axis(&spec.x, &spec.name)?;
            let y = spec.y.as_ref().map(|y| compile_axis(y, &spec.name)).transpose()?;
            let shm_name = self.shm_name(&spec.name);
            let histo_config = HistoConfig {
                nx: x.bins as usize,
                ny: y.as_ref().map_or(1, |y| y.bins as usize),
                max_nt: 1,
                max_ni: 0,
            };
            result.push(PreparedHisto { name: spec.name.clone(), filter, x, y, shm_name, histo_config });
        }
        Ok(result)
    }

    /// Custom `#[param(has_setter = true)]` setter for `histos`: validates
    /// everything first (old state is untouched on error), then unlinks all
    /// current shm segments and creates fresh ones for the new list.
    fn set_histos(&mut self, value: Vec<HistoSpec>) -> UResult<()> {
        let prepared = self.prepare(&value)?;
        for h in &self.compiled {
            let _ = nix::sys::mman::shm_unlink(h.shm_name.as_bytes());
        }
        let mut new_compiled = Vec::with_capacity(prepared.len());
        for p in prepared {
            let shm = ShmInterface::create(&p.shm_name, &p.histo_config)
                .with_context(|| format!("Creating shm segment for histogram {:?}", p.name))?;
            new_compiled.push(CompiledHisto {
                filter: p.filter, x: p.x, y: p.y, shm_name: p.shm_name, shm,
            });
        }
        self.compiled = new_compiled;
        self.histos = value;
        Ok(())
    }
}

impl Output for AuxHistoOutput {
    fn from_config(common: &OutputCommon, config: toml::Table) -> UResult<Self> {
        let config: AuxHistoConfig = config.try_into().context("Parsing aux_histo output config")?;
        let mut output = AuxHistoOutput {
            ipc_name: common.ipc_name.clone(),
            name: common.name,
            enabled: config.enabled,
            histos: Vec::new(),
            compiled: Vec::new(),
        };
        output.set_histos(config.histos).context("Building initial auxiliary histograms")?;
        Ok(output)
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        if !self.enabled {
            return Ok(());
        }
        for ev in events {
            for h in &mut self.compiled {
                if h.filter.eval(ev) == 0 {
                    continue;
                }
                let Some(bx) = bin_index(h.x.expr.eval(ev), h.x.min, h.x.max, h.x.bins) else { continue };
                let by = match &h.y {
                    Some(y) => match bin_index(y.expr.eval(ev), y.min, y.max, y.bins) {
                        Some(b) => b,
                        None => continue,
                    },
                    None => 0,
                };
                h.shm.add_histo(EventHisto { x: bx, y: by, t: 0, i: 0 });
            }
        }
        Ok(())
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        for h in &mut self.compiled {
            h.shm.set_run_id(run);
        }
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        Ok(())
    }

    fn handle_clear(&mut self) -> UResult<()> {
        for h in &mut self.compiled {
            h.shm.clear_histo();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use crate::event::{test_utils, Amplitude};
    use crate::params::ParamMap;
    use crate::shm::ShmGuard;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_ipc() -> String {
        format!("umami_auxtest_{}_{}", COUNTER.fetch_add(1, Ordering::SeqCst), std::process::id())
    }

    fn test_common(ipc_name: &str, out_name: &str) -> OutputCommon {
        let (_send, recv) = crate::channel::unbounded();
        OutputCommon::new(ModuleId::new(out_name.into()), ipc_name.into(), recv, None)
    }

    #[test]
    fn test_1d_histogram_filters_and_bins() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            filter = "evtype == neutron"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        let mut ev1 = test_utils::neutron(0, 0);
        ev1.ampl = Amplitude(1); // bin 0 of 4, range [0,8) -> width 2 -> bin 0
        let mut ev2 = test_utils::neutron(0, 0);
        ev2.ampl = Amplitude(5); // bin 2
        let filtered_out = test_utils::tzero(0); // not Neutron

        output.handle_events(&[ev1, ev2, filtered_out]).unwrap();

        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        let data = shm.histo_data();
        assert_eq!(data[0], 1);
        assert_eq!(data[2], 1);
        assert_eq!(data.iter().sum::<u32>(), 2);
    }

    #[test]
    fn test_2d_histogram_and_bit_slice_expr() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "xy"
            x = { expr = "raw_0[0..12:signed]", bins = 4, min = -8, max = 7 }
            y = { expr = "raw_1[0..12:signed]", bins = 4, min = -8, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_xy"));

        let mut ev = test_utils::neutron(0, 0);
        ev.raw = (2, 6); // x bin: (2-(-8))*4/16 = 2, y bin: (6-(-8))*4/16 = 3
        output.handle_events(&[ev]).unwrap();

        let shm = ShmInterface::open(&format!("{ipc}_aux_xy")).unwrap();
        let data = shm.histo_data();
        // layout is [t][y][x], nx=4 -> offset = y*4 + x
        assert_eq!(data[3 * 4 + 2], 1);
        assert_eq!(data.iter().sum::<u32>(), 1);
    }

    #[test]
    fn test_out_of_range_value_is_dropped_not_clamped() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        let mut ev = test_utils::neutron(0, 0);
        ev.ampl = Amplitude(100); // way out of [0,7]
        output.handle_events(&[ev]).unwrap();

        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        assert_eq!(shm.histo_data().iter().sum::<u32>(), 0);
    }

    #[test]
    fn test_enabled_toggle_skips_event_processing() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));
        let ev = test_utils::neutron(0, 0);

        let mut disable = ParamMap::new();
        disable.insert("enabled".into(), serde_json::json!(false));
        output.update_params(ModuleId::new("aux".into()), disable).unwrap();
        output.handle_events(&[ev]).unwrap();
        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        assert_eq!(shm.histo_data().iter().sum::<u32>(), 0);

        // re-enabling doesn't require recreating the segment or histogram list
        let mut enable = ParamMap::new();
        enable.insert("enabled".into(), serde_json::json!(true));
        output.update_params(ModuleId::new("aux".into()), enable).unwrap();
        output.handle_events(&[ev]).unwrap();
        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        assert_eq!(shm.histo_data().iter().sum::<u32>(), 1);
    }

    #[test]
    fn test_invalid_expr_rejected_at_config_time() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "bad"
            x = { expr = "not_a_real_field", bins = 4, min = 0, max = 8 }
        "#).unwrap();
        assert!(AuxHistoOutput::from_config(&common, cfg).is_err());
    }

    #[test]
    fn test_set_params_rebuilds_and_rejects_bad_update_without_disturbing_state() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard1 = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        // a bad update is rejected, old histogram list is untouched
        let mut bad = ParamMap::new();
        bad.insert("histos".into(), serde_json::json!([
            { "name": "bad", "x": { "expr": "nonsense", "bins": 4, "min": 0, "max": 7 } }
        ]));
        assert!(output.update_params(ModuleId::new("aux".into()), bad).is_err());
        assert_eq!(output.histos.len(), 1);
        assert_eq!(output.histos[0].name, "amp");

        // a good update replaces the list and creates a new segment
        let mut good = ParamMap::new();
        good.insert("histos".into(), serde_json::json!([
            { "name": "chan", "x": { "expr": "channel", "bins": 2, "min": 0, "max": 1 } }
        ]));
        output.update_params(ModuleId::new("aux".into()), good).unwrap();
        let _guard2 = ShmGuard::for_name(format!("{ipc}_aux_chan"));
        assert_eq!(output.histos.len(), 1);
        assert_eq!(output.histos[0].name, "chan");
        // the old segment is gone (unlinked)
        assert!(ShmInterface::open(&format!("{ipc}_aux_amp")).is_err());
    }

    #[test]
    fn test_clear_zeroes_all_histograms() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        let ev = test_utils::neutron(0, 0);
        output.handle_events(&[ev]).unwrap();
        output.handle_clear().unwrap();

        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        assert_eq!(shm.histo_data().iter().sum::<u32>(), 0);
    }

    #[test]
    fn test_start_of_run_sets_run_id() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        output.handle_start_of_run("run_042").unwrap();
        let shm = ShmInterface::open(&format!("{ipc}_aux_amp")).unwrap();
        let run_id = std::str::from_utf8(&shm.run_id).unwrap();
        assert!(run_id.starts_with("run_042"));
    }

    #[test]
    fn test_get_params_reports_current_state() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "amp"
            x = { expr = "ampl", bins = 4, min = 0, max = 7 }
        "#).unwrap();
        let output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_amp"));

        let params = output.get_params().unwrap();
        assert_eq!(params["enabled"]["value"], true);
        assert_eq!(params["histos"]["value"][0]["name"], "amp");
    }

    #[test]
    fn test_max_is_inclusive() {
        let ipc = unique_ipc();
        let common = test_common(&ipc, "aux");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "chan"
            x = { expr = "channel", bins = 56, min = 0, max = 55 }
        "#).unwrap();
        let mut output = AuxHistoOutput::from_config(&common, cfg).unwrap();
        let _guard = ShmGuard::for_name(format!("{ipc}_aux_chan"));

        for channel in [0u32, 55] {
            let mut ev = test_utils::neutron(0, 0);
            ev.channel = crate::event::ChannelId(channel);
            output.handle_events(&[ev]).unwrap();
        }

        let shm = ShmInterface::open(&format!("{ipc}_aux_chan")).unwrap();
        let data = shm.histo_data();
        assert_eq!(data[0], 1, "channel 0 should land in bin 0");
        assert_eq!(data[55], 1, "channel 55 (== max) should land in bin 55, not be dropped");
        assert_eq!(data.iter().sum::<u32>(), 2);
    }
}
