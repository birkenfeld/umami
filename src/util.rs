// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::net::ToSocketAddrs;
use anyhow::anyhow;
use crate::error::UResult;

/// Resolves a string to the corresponding IPv4 socket address.
pub fn resolve(addr: &str) -> UResult<std::net::SocketAddr> {
    Ok(addr
       .to_socket_addrs()
       .map_err(|e| anyhow!("Invalid address '{}': {}", addr, e))?
       .filter(|a| a.is_ipv4())
       .next().ok_or_else(|| anyhow!("No addresses found for '{}'", addr))?
    )
}
