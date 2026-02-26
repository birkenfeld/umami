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
pub enum ModuleConfig {
    GE(GEConfig),
    Canon(CanonConfig),
    Mesy(MesyConfig),
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub modules: BTreeMap<String, ModuleConfig>,
    #[serde(default = "default_shm_name")]
    pub shm_name: String,
    #[serde(default)]
    pub debug: bool,
}

fn default_shm_name() -> String {
    "umami".into()
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
