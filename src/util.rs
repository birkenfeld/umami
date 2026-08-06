// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
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
       .map_err(|e| anyhow!("Invalid address '{addr}': {e}"))?
       .find(|a| a.is_ipv4())
       .ok_or_else(|| anyhow!("No addresses found for '{addr}'"))?
    )
}

/// Deserializes a `BTreeMap<K, T>` from a string-keyed map, as TOML (and
/// JSON) always represent it on the wire -- the `toml` crate can't
/// deserialize a non-string-keyed map directly.
pub fn deserialize_map_with_key<'de, D, K, T>(deserializer: D) -> Result<BTreeMap<K, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: std::str::FromStr + Ord,
    T: Deserialize<'de>,
{
    let string_keyed: BTreeMap<String, T> = BTreeMap::deserialize(deserializer)?;
    string_keyed.into_iter()
        .map(|(k, v)| {
            k.parse::<K>()
                .map(|idx| (idx, v))
                .map_err(|_| serde::de::Error::custom(
                    format!("Invalid index key {k:?}, expected a number")))
        })
        .collect()
}

/// Wait for termination signals.
pub fn wait_for_signal() -> io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    signals.wait();
    lprintln!(INFO, "Termination signal received, shutting down.");
    Ok(())
}
