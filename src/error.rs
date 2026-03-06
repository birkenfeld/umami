// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use thiserror::Error;

pub type UResult<T> = Result<T, UError>;

// TODO: decide if we need specific error types
// Which errors need to be handled specially?
#[derive(Debug, Error)]
pub enum UError {
    #[error("{0:#}")]
    Other(#[from] anyhow::Error),
}
