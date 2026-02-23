// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use thiserror::Error;

pub type UResult<T> = Result<T, UError>;

#[derive(Debug, Error)]
pub enum UError {
    #[error("Failed to initialize event source: {0}")]
    SourceInit(#[source] std::io::Error),
    #[error("Failed to read event from input: {0}")]
    ReadInput(#[from] std::io::Error),
    #[error("Unspecified error: {0}")]
    Other(#[from] anyhow::Error),
}
