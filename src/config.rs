// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum InputConfig {
    GE {
        addr: String,
        ts: bool,
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub inputs: BTreeMap<String, InputConfig>,
}
