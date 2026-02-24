// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ModuleConfig {
    GE {
        addr: String,
        ts: bool,
    },
    Canon {
        addr: String,
        gate: bool,
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub modules: BTreeMap<String, ModuleConfig>,
    // TODO: loglevel
}
