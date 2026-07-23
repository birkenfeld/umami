// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use anyhow::Context;
use serde::Deserialize;
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
    pub mcpd_id: u8,
    pub cells: BTreeMap<usize, MesyCellConfig>,
    pub modules: BTreeMap<usize, MesyModuleConfig>,
}

#[derive(Debug, Deserialize)]
pub struct MesyCellConfig {
    // TODO values are more restricted
    pub source: u16,
    pub compare: u16,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MesyModuleConfig {
    // TODO better types
    // TODO amp mode, pulser
    Mpsd { threshold: u16, gain: u16 },
    Mstd { threshold: u16, gain: u16 },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpecificInputConfig {
    GE(GEConfig),
    Canon(CanonConfig),
    Mesy(MesyConfig),
    #[cfg(test)]
    Test(TestInputConfig),
}

/// Config for a synthetic input backend used only in pipeline tests: it
/// generates one Neutron event for every (x, y) cell in `0..nx` x `0..ny`,
/// so tests can assert an exact, hand-computed histogram instead of relying
/// on golden data files.
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
    pub max_ni: usize,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub inputs: BTreeMap<String, InputConfig>,
    pub input_recipes: BTreeMap<String, RecipeConfig>,
    pub outputs: Option<BTreeMap<String, OutputConfig>>,
    pub process_modes: ProcessModesConfig,
    pub histogram: HistoConfig,
    #[serde(default = "default_ipc_name")]
    pub ipc_name: String,
    pub raw_dir: Option<PathBuf>,
    #[serde(default)]
    pub debug: bool,
    #[serde(skip)]
    pub filename: PathBuf,
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
    fn test_source_config_ip() {
        let cfg: SourceConfig = toml::from_str(r#"val = "localhost:50001""#)
            .map(|t: toml::Table| t["val"].clone().try_into().unwrap()).unwrap();
        match cfg {
            SourceConfig::IP(s) => assert_eq!(s, "localhost:50001"),
            _ => panic!("expected IP"),
        }
    }

    #[test]
    fn test_source_config_file() {
        let cfg: SourceConfig = toml::from_str(r#"val = "/path/to/file""#)
            .map(|t: toml::Table| t["val"].clone().try_into().unwrap()).unwrap();
        match cfg {
            SourceConfig::File(s) => assert_eq!(s, "/path/to/file"),
            _ => panic!("expected File"),
        }
    }

    #[test]
    fn test_source_config_ip_without_colon_falls_to_file() {
        let table: toml::Table = toml::from_str(r#"val = "localhost""#).unwrap();
        let cfg: SourceConfig = table["val"].clone().try_into().unwrap();
        // untagged: IP fails (no colon), falls through to File
        assert!(matches!(cfg, SourceConfig::File(s) if s == "localhost"));
    }

    #[test]
    fn test_ge_config() {
        let cfg: GEConfig = toml::from_str(r#"
            source = "localhost:50001"
            timestamper = true
        "#).unwrap();
        assert!(matches!(cfg.source, SourceConfig::IP(_)));
        assert!(cfg.timestamper);
    }

    #[test]
    fn test_ge_config_defaults() {
        let cfg: GEConfig = toml::from_str(r#"
            source = "/path/to/file"
        "#).unwrap();
        assert!(!cfg.timestamper);
    }

    #[test]
    fn test_recipe_config() {
        let cfg: RecipeConfig = toml::from_str(r#"
            type = "histo_std"
            bin_x = 2
        "#).unwrap();
        assert_eq!(cfg.r#type, "histo_std");
        assert_eq!(cfg.config.get("bin_x").and_then(|v| v.as_integer()), Some(2));
    }

    #[test]
    fn test_histo_config() {
        let cfg: HistoConfig = toml::from_str(r#"
            nx = 100
            ny = 200
            max_nt = 50
            max_ni = 8
        "#).unwrap();
        assert_eq!(cfg.nx, 100);
        assert_eq!(cfg.ny, 200);
        assert_eq!(cfg.max_nt, 50);
        assert_eq!(cfg.max_ni, 8);
    }

    #[test]
    fn test_process_modes_config() {
        let cfg: ProcessModesConfig = toml::from_str(r#"
            default = "std"
            std = { type = "histo_std" }
        "#).unwrap();
        assert_eq!(cfg.default, "std");
        assert!(cfg.recipes.contains_key("std"));
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
            none = { type = "none" }

            [process_modes]
            default = "std"
            std = { type = "histo_std" }

            [histogram]
            nx = 100
            ny = 100
            max_nt = 10
            max_ni = 0
        "#).unwrap();
        assert_eq!(cfg.inputs.len(), 1);
        assert!(cfg.inputs.contains_key("main"));
        assert_eq!(cfg.ipc_name, "umami");
        assert!(!cfg.debug);
    }

    #[test]
    fn test_config_optional_outputs() {
        let cfg: Config = toml::from_str(r#"
            [inputs.main]
            id = 0
            recipe = "none"
            type = "ge"
            source = "localhost:50001"

            [input_recipes]
            none = { type = "none" }

            [process_modes]
            default = "std"
            std = { type = "histo_std" }

            [histogram]
            nx = 100
            ny = 100
            max_nt = 10
            max_ni = 0
        "#).unwrap();
        assert!(cfg.outputs.is_none());
    }
}
