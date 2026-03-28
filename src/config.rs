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
}

#[derive(Debug, Deserialize)]
pub struct InputConfig {
    pub id: u16,
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
