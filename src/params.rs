// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Parameters for inputs, outputs, and post-processing steps.

use serde::Serialize;
use crate::command::ModuleId;
use crate::error::UResult;

// The bulk of the functionality lives here.
pub use umami_derive::HasParams;

pub type ParamMap = serde_json::Map<String, serde_json::Value>;

pub trait HasParams {
    /// `full`: include `datatype`/`help`/`readonly`/`runtime_only` metadata
    /// alongside each value, instead of just `{"value": ...}` -- see
    /// [`ParamInfo`].
    fn get_params(&self, full: bool) -> UResult<ParamMap>;
    fn update_params(&mut self, name: ModuleId, params: ParamMap) -> UResult<()>;
}

#[derive(Serialize, Debug)]
pub struct ParamInfo {
    pub datatype: String,
    pub help: String,
    pub readonly: bool,
    /// Not written back to the config file by `SaveConfig`.
    pub runtime_only: bool,
    pub value: serde_json::Value,
}

/// The synthetic `<module>._info` entry identifying a module's kind and
/// configured type (e.g. `{"kind": "input", "type": "mesy"}`), inserted
/// alongside a module's own params only in full replies -- see
/// [`HasParams::get_params`].
pub fn info_entry(kind: &str, type_name: &str) -> serde_json::Value {
    serde_json::json!({"kind": kind, "type": type_name})
}
