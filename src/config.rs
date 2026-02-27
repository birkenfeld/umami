// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;

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
    #[serde(default)]
    pub gatenet: bool,
}

#[derive(Debug, Deserialize)]
pub struct MesyConfig {
    pub source: SourceConfig,
    pub local: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpecificModuleConfig {
    GE(GEConfig),
    Canon(CanonConfig),
    Mesy(MesyConfig),
}

#[derive(Debug, Deserialize)]
pub struct ModuleConfig {
    pub id: u16,
    pub recipe: String,
    #[serde(flatten)]
    pub specific: SpecificModuleConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RecipeConfig {
    pub r#type: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Deserialize)]
pub struct PostConfig {
    #[serde(default)]
    pub recipe: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoConfig {
    pub nx: usize,
    pub ny: usize,
    pub max_nt: usize,
    #[serde(default = "default_tbin")]
    pub default_tbin: f64,
    #[serde(default)]
    pub default_tdelay: f64,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub modules: BTreeMap<String, ModuleConfig>,
    pub recipes: BTreeMap<String, RecipeConfig>,
    pub postprocess: PostConfig,
    pub histogram: HistoConfig,
    #[serde(default = "default_shm_name")]
    pub shm_name: String,
    #[serde(default)]
    pub debug: bool,
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

fn default_shm_name() -> String {
    "umami".into()
}

fn default_tbin() -> f64 {
    1e-6 // seconds
}
