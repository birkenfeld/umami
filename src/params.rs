// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Parameters for inputs, outputs, and post-processing steps.

use serde::Serialize;
use crate::error::UResult;

pub use umami_derive::HasParams;

pub type ParamMap = serde_json::Map<String, serde_json::Value>;

pub trait HasParams {
    fn get_params(&self) -> UResult<ParamMap>;
    fn update_params(&mut self, name: &str, params: ParamMap) -> UResult<()>;
}

#[derive(Serialize, Debug)]
pub struct ParamInfo {
    pub datatype: String,
    pub help: String,
    pub value: serde_json::Value,
}
