// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use thiserror::Error;

pub type UResult<T> = Result<T, UError>;

#[derive(Debug, Error)]
pub enum UError {
    #[error(transparent)]
    Other(#[from] anyhow::Error),
    #[error("Data from file exhausted")]
    NoMoreData,
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    #[test]
    fn chain_is_not_duplicated_across_a_context_rewrap() {
        fn inner() -> super::UResult<()> {
            Err(std::io::Error::from_raw_os_error(2))
                .with_context(|| "Opening source file \"x\"".to_string())?
        }
        fn outer() -> super::UResult<()> {
            inner().context("Error setting parameter replay_file")?;
            Ok(())
        }

        let message = format!("{:#}", outer().unwrap_err());
        assert_eq!(
            message,
            "Error setting parameter replay_file: Opening source file \"x\": \
             No such file or directory (os error 2)"
        );
    }
}
