// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::net::ToSocketAddrs;
use anyhow::anyhow;
use serde::Deserialize;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use crate::lprintln;
use crate::error::UResult;

/// Resolves a string to the corresponding IPv4 socket address.
pub fn resolve(addr: &str) -> UResult<std::net::SocketAddr> {
    Ok(addr
       .to_socket_addrs()
       .map_err(|e| anyhow!("Invalid address '{}': {}", addr, e))?
       .find(|a| a.is_ipv4())
       .ok_or_else(|| anyhow!("No addresses found for '{}'", addr))?
    )
}

/// Wait for termination signals.
pub fn wait_for_signal() -> io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    signals.wait();
    lprintln!(INFO, "Termination signal received, shutting down.");
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum FalseOr<T> {
    #[default]
    #[serde(deserialize_with = "deserialize_false")]
    False,
    Value(T),
}

impl<T> FalseOr<T> {
    pub fn is_false(&self) -> bool {
        matches!(self, FalseOr::False)
    }
}

fn deserialize_false<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    if bool::deserialize(deserializer)? {
        Err(serde::de::Error::custom("Expected 'false'"))
    } else {
        Ok(())
    }
}
