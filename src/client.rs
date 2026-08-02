// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use anyhow::{bail, Context};
use uds::UnixDatagramExt;

use crate::command::{Command, CommandReply, RECV_BUFFER_SIZE};

static CLIENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Client {
    sock: UnixDatagram,
}

impl Client {
    pub fn new(ipc_name: &str) -> anyhow::Result<Self> {
        let unique_name = format!(
            "umami-{}-{}", std::process::id(),
            CLIENT_COUNTER.fetch_add(1, Ordering::SeqCst),
        );
        let my_addr = uds::UnixSocketAddr::from_abstract(unique_name.as_bytes())
            .context("Creating abstract socket address")?;
        let target_addr = uds::UnixSocketAddr::from_abstract(ipc_name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = UnixDatagram::bind_unix_addr(&my_addr)
            .context("Creating Unix socket")?;
        sock.set_read_timeout(Some(Duration::from_secs(2)))
            .context("Setting socket read timeout")?;
        sock.connect_to_unix_addr(&target_addr)
            .context("Connecting Unix socket")?;
        Ok(Self { sock })
    }

    pub fn send(&self, cmd: &Command) -> anyhow::Result<CommandReply> {
        let cmd_json = serde_json::to_string(cmd).context("Serializing command to JSON")?;
        self.sock.send(cmd_json.as_bytes()).context("Sending command")?;
        let mut buf = [0u8; RECV_BUFFER_SIZE];
        let n = match self.sock.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                bail!("No reply received (timeout)")
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                bail!("No reply received (timeout)")
            }
            Err(e) => Err(e).context("Receiving reply")?,
        };
        let reply_buf = std::str::from_utf8(&buf[..n])
            .context("Reply is not valid UTF-8")?;
        serde_json::from_str(reply_buf)
            .context("Parsing reply JSON")
    }
}
