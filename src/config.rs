// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ModuleConfig {
    GE {
        addr: String,
        #[serde(default)]
        ts: bool,
    },
    Canon {
        addr: String,
        #[serde(default)]
        gate: bool,
    },
    Mesy {
        addr: String,
        local: String,
    }
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
