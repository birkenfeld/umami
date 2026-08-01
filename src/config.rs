// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use crate::error::UResult;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SourceConfig {
    IP(#[serde(deserialize_with = "deserialize_ip")] String),
    File(String),
}

#[derive(Debug, Deserialize)]
pub struct GEConfig {
    pub source: SourceConfig,
    #[serde(default)]
    pub timestamper: bool,
}

#[derive(Debug, Deserialize)]
pub struct CanonConfig {
    pub source: SourceConfig,
    pub channel_offset: u32,
    #[serde(default)]
    pub gatenet: bool,
}

#[derive(Debug, Deserialize)]
pub struct MesyConfig {
    pub local: SourceConfig,
    pub remote: String,
    pub is_master: bool,
    /// Sync-bus termination. Forced on for the master.
    pub terminate: bool,
    /// External synchronisation input, only meaningful when `is_master`.
    #[serde(default)]
    pub ext_sync: bool,
    /// Negotiate amplitude data (TPA) into the transmission mode if
    /// supported, vs. capping at time+position (TP) for lower overhead.
    #[serde(default = "default_true")]
    pub transmit_ampl: bool,
    pub mcpd_id: u8,
    pub cells: BTreeMap<usize, MesyCellConfig>,
    pub modules: BTreeMap<usize, MesyModuleConfig>,
}

/// A cell's trigger source: which physical/logical signal counts into it.
#[repr(u16)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CellTrigger {
    None = 0,
    Aux1 = 1,
    Aux2 = 2,
    Aux3 = 3,
    Aux4 = 4,
    Digital1 = 5,
    Digital2 = 6,
    Compare = 7,
}

/// A bit index into the MCPD's compare/status register: 0-20 select one of
/// its 21 status bits, 21 is the counter-overflow pseudo-bit, 22 is the
/// rising-edge pseudo-bit. Only meaningful when a cell's `source` is
/// `CellTrigger::Compare`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CompareBit(u16);

impl CompareBit {
    pub fn new(value: u16) -> anyhow::Result<Self> {
        if value > 22 {
            Err(anyhow!("Compare bit must be 0-22, got {value}"))?;
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for CompareBit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de>
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MesyCellConfig {
    pub source: CellTrigger,
    pub compare: CompareBit,
}

/// An MPSD's gain, either the same for every channel or given per channel
/// (one value per tube).
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MesyGain {
    Uniform(u16),
    PerChannel([u16; 8]),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MesyModuleConfig {
    // TODO better types
    // TODO amp mode
    Mpsd { threshold: u16, gain: MesyGain },
    Mstd { threshold: u16, gain: u16 },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpecificInputConfig {
    GE(GEConfig),
    Canon(CanonConfig),
    Mesy(MesyConfig),
    #[cfg(feature = "jumiom")]
    Jumiom(JumiomConfig),
    #[cfg(test)]
    Test(TestInputConfig),
}

/// Acquisition mode for the Jumiom PSD (selects both the hardware mode set
/// up via `jumpsd_set_*_mode` and the raw word-stream decoding). `Tof2` is
/// intentionally not supported (see `src/input/jumiom/decode.rs`).
#[cfg(feature = "jumiom")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JumiomMode {
    Tof1,
    Raw,
    Ramp,
}

#[cfg(feature = "jumiom")]
#[derive(Debug, Deserialize)]
pub struct JumiomConfig {
    /// Device number, i.e. `/dev/jumpsd_d<device>`.
    pub device: i32,
    pub mode: JumiomMode,
    /// Hardware calibration to push at acquisition start, matching what
    /// `jumiom_dma_wrapper`'s startup sequence used to write from
    /// `globalData.gp` when `loadCard` was set. If unset, umami leaves the
    /// hardware's current settings untouched (like `loadCard = 0`).
    #[serde(default)]
    pub calibration: Option<JumiomCalibration>,
}

/// Hardware calibration values for the Jumiom PSD, pushed via the
/// `jumpsd_write_*` API in `jumpsd_setup_callback`
/// (see `src/input/jumiom.rs`). Grouped to match how they're set together in
/// the field's `entangle` config (`jumiom_det.ImageChannel`).
#[cfg(feature = "jumiom")]
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct JumiomCalibration {
    /// Upper/lower/gate ADC thresholds (`jumpsd_write_threshold` levels 0..2).
    pub thresholds: [i32; 3],
    /// Gain potentiometer setting per ADC channel (`jumpsd_write_poti`).
    pub poti: [i32; 4],
    /// DAC offset per ADC channel, single-ended (`jumpsd_write_dac`).
    pub dac1: [i32; 4],
    /// DAC offset per ADC channel, differential (`jumpsd_write_dac2`).
    pub dac2: [i32; 4],
    /// Pileup rejection count.
    pub pileup: i32,
    /// Monitor timer reset delay [us] (`jumpsd_write_monitor_delay`).
    /// Monitor recording is always enabled, regardless of this block.
    #[serde(default)]
    pub monitor_delay: i32,
    /// Chopper timer reset delay [us] (`jumpsd_write_chopper_delay`).
    /// Chopper recording is always enabled, regardless of this block.
    #[serde(default)]
    pub chopper_delay: i32,
}

/// Config for a synthetic input backend used only in pipeline tests: it
/// generates one Neutron event for every (x, y) cell in `0..nx` x `0..ny`,
/// so tests can assert an exact histogram.
#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct TestInputConfig {
    pub nx: u16,
    pub ny: u16,
}

#[derive(Debug, Deserialize)]
pub struct InputConfig {
    pub recipe: String,
    #[serde(flatten)]
    pub specific: SpecificInputConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RecipeConfig {
    pub r#type: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OutputConfig {
    pub r#type: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Deserialize)]
pub struct ProcessModesConfig {
    pub default: String,
    #[serde(flatten)]
    pub recipes: BTreeMap<String, RecipeConfig>,
}

#[derive(Debug, Deserialize)]
pub struct HistoConfig {
    pub nx: usize,
    pub ny: usize,
    pub max_nt: usize,
    #[serde(default)]
    pub max_ni: usize,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Input modules, by name.
    pub inputs: BTreeMap<String, InputConfig>,
    /// Recipes for processing input events, by name.  Each input module
    /// specifies exactly one recipe to use for its events - usually it
    /// is the same for all inputs of a certain type.
    pub input_recipes: BTreeMap<String, RecipeConfig>,
    /// Processing modes, by name.  Each processing mode is a recipe
    /// that is applied after events are read and sorted.
    pub process_modes: ProcessModesConfig,
    /// Histogram configuration (size, max events per bin, etc.).
    pub histogram: HistoConfig,
    /// Output modules, by name.
    pub outputs: Option<BTreeMap<String, OutputConfig>>,
    /// IPC name - shared memory segment and Unix socket name.
    #[serde(default = "default_ipc_name")]
    pub ipc_name: String,
    /// Optional name of the detector config, cosmetic only.
    pub name: Option<String>,
    /// Optional path to a directory where raw event dumps are written.
    pub raw_dir: Option<PathBuf>,
    /// Debug mode: print extra info to stdout.
    #[serde(default)]
    pub debug: bool,
    /// User-defined expression aliases for aux histograms.
    #[serde(default)]
    pub expr_aliases: BTreeMap<String, ExprAliasConfig>,
    /// Path to the config file, filled in by `load_config()`.
    #[serde(skip)]
    pub filename: PathBuf,
}

/// Deserialized from either a plain string (the expression, no help text) or a
/// table with an optional `help`.
#[derive(Debug, Clone)]
pub struct ExprAliasConfig {
    pub expr: String,
    pub help: String,
}

impl<'de> Deserialize<'de> for ExprAliasConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de>
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum Repr {
            Short(String),
            Full {
                expr: String,
                #[serde(default)]
                help: String,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Short(expr) => ExprAliasConfig { expr, help: String::new() },
            Repr::Full { expr, help } => ExprAliasConfig { expr, help },
        })
    }
}

fn deserialize_ip<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.contains(':') {
        Ok(s)
    } else {
        Err(serde::de::Error::custom("Expected an IP address (string containing ':')"))
    }
}

fn default_ipc_name() -> String {
    "umami".into()
}

fn default_true() -> bool {
    true
}

pub fn load_config(path: &Path) -> UResult<Config> {
    let mut config: Config = toml::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file {:?}", path.display()))?
    ).with_context(|| format!("Failed to parse config file {:?}", path.display()))?;
    config.filename = path.canonicalize().context("Failed to get canonical config path")?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_config_deserialize() {
        let tbl: toml::Table = toml::from_str(r#"
            val1 = "localhost:50001"
            val2 = "/path/to/file"
        "#).unwrap();
        let cfg: SourceConfig = tbl["val1"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::IP(s) if s == "localhost:50001"));
        let cfg: SourceConfig = tbl["val2"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::File(s) if s == "/path/to/file"));
    }

    #[test]
    fn test_full_config_minimal() {
        let cfg: Config = toml::from_str(r#"
            [inputs.main]
            id = 0
            recipe = "none"
            type = "ge"
            source = "localhost:50001"

            [input_recipes]
            none = { type = "none", blah = 5 }

            [process_modes]
            default = "std"
            std = { type = "histo_std", bin_x = 2 }

            [histogram]
            nx = 100
            ny = 100
            max_nt = 10
        "#).unwrap();
        assert_eq!(cfg.inputs.len(), 1);
        assert!(cfg.inputs.contains_key("main"));
        assert_eq!(cfg.ipc_name, "umami");
        assert!(!cfg.debug);
        assert!(cfg.outputs.is_none());
        assert_eq!(cfg.process_modes.default, "std");
        assert!(cfg.process_modes.recipes.contains_key("std"));
        let proc = cfg.process_modes.recipes.get("std").unwrap();
        assert_eq!(proc.r#type, "histo_std");
        assert_eq!(proc.config.get("bin_x").and_then(|v| v.as_integer()), Some(2));
        let rec = cfg.input_recipes.get("none").unwrap();
        assert_eq!(rec.r#type, "none");
        assert_eq!(rec.config.get("blah").and_then(|v| v.as_integer()), Some(5));
    }
}
