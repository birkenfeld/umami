// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use anyhow::Context;
use thiserror::Error;
use uds::UnixDatagramExt;

use crate::command::{Command, CommandReply, RECV_BUFFER_SIZE};

static CLIENT_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Default read timeout, matching the server's own cap on how long it waits
/// for a reply from inputs/postprocessor (`command.rs`'s `REPLY_TIMEOUT`) --
/// so a wedged module and a client giving up happen at roughly the same time.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("No reply received (timeout)")]
    Timeout,
    #[error(transparent)]
    Connection(#[from] io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub struct Client {
    ipc_name: String,
    timeout: Duration,
    sock: Option<UnixDatagram>,
}

impl Client {
    pub fn new(ipc_name: &str) -> anyhow::Result<Self> {
        Self::with_timeout(ipc_name, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(ipc_name: &str, timeout: Duration) -> anyhow::Result<Self> {
        let mut client = Self { ipc_name: ipc_name.to_string(), timeout, sock: None };
        client.reconnect()?;
        Ok(client)
    }

    pub fn connected(&self) -> bool {
        self.sock.is_some()
    }

    /// (Re)binds a fresh local abstract address and, if the target is up,
    /// connects to it.
    ///
    /// Every reconnect gets a fresh local address: otherwise, a client that
    /// just timed out on one command could still receive that command's reply
    /// after sending the next one, and misread it as the new command's answer.
    ///
    /// Connection failure is not an error here: it just leaves `connected()`
    /// false, for `send`/an explicit `reconnect` call to retry later.
    pub fn reconnect(&mut self) -> anyhow::Result<()> {
        self.sock = None;
        let unique_name = format!(
            "umami-{}-{}", std::process::id(),
            CLIENT_COUNTER.fetch_add(1, Ordering::SeqCst),
        );
        let my_addr = uds::UnixSocketAddr::from_abstract(unique_name.as_bytes())
            .context("Creating abstract socket address")?;
        let target_addr = uds::UnixSocketAddr::from_abstract(self.ipc_name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = UnixDatagram::bind_unix_addr(&my_addr)
            .context("Creating Unix socket")?;
        sock.set_read_timeout(Some(self.timeout))
            .context("Setting socket read timeout")?;
        if sock.connect_to_unix_addr(&target_addr).is_ok() {
            self.sock = Some(sock);
        }
        Ok(())
    }

    /// Main entry point: sends a command and waits for a reply.
    ///
    /// If the socket is not connected, attempts a reconnect first.  If the send
    /// fails, the socket is dropped so that the next send will try to reconnect
    /// again.
    pub fn send(&mut self, cmd: &Command) -> Result<CommandReply, ClientError> {
        if self.sock.is_none() {
            self.reconnect()?;
        }
        if self.sock.is_none() {
            return Err(ClientError::Connection(io::Error::new(
                io::ErrorKind::NotConnected,
                format!("{:?} is not reachable", self.ipc_name),
            )));
        }
        let result = self.send_once(cmd);
        if result.is_err() {
            // force a fresh address on the next attempt rather than retrying
            // with a socket that may be desynced
            self.sock = None;
        }
        result
    }

    /// Sends a command and waits for a reply, without attempting to reconnect.
    fn send_once(&self, cmd: &Command) -> Result<CommandReply, ClientError> {
        let sock = self.sock.as_ref().expect("connected by send()");
        let cmd_json = serde_json::to_string(cmd)
            .context("Serializing command to JSON")?;
        sock.send(cmd_json.as_bytes())?;
        let mut buf = [0u8; RECV_BUFFER_SIZE];
        let n = match sock.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock |
                               io::ErrorKind::TimedOut) => {
                return Err(ClientError::Timeout);
            }
            Err(e) => return Err(e.into()),
        };
        let reply_buf = std::str::from_utf8(&buf[..n])
            .context("Reply is not valid UTF-8")?;
        serde_json::from_str(reply_buf)
            .context("Parsing reply JSON")
            .map_err(ClientError::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_name(tag: &str) -> String {
        format!("umami_clienttest_{tag}_{}_{}",
                std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
    }

    fn bind_fake_server(name: &str) -> UnixDatagram {
        let addr = uds::UnixSocketAddr::from_abstract(name.as_bytes()).unwrap();
        UnixDatagram::bind_unix_addr(&addr).unwrap()
    }

    #[test]
    fn test_client_sends_and_receives() {
        let name = unique_name("ok");
        let server = bind_fake_server(&name);
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("\"clear\""));
            server.send_to_addr(br#"{"result":"ok"}"#, &peer).unwrap();
        });

        let mut client = Client::new(&name).unwrap();
        assert!(client.connected());
        assert!(matches!(client.send(&Command::Clear).unwrap(), CommandReply::Ok));
    }

    #[test]
    fn test_client_timeout_marks_disconnected() {
        let name = unique_name("silent");
        let _server = bind_fake_server(&name); // reachable, but never replies

        let mut client = Client::with_timeout(&name, Duration::from_millis(50)).unwrap();
        assert!(matches!(client.send(&Command::Clear), Err(ClientError::Timeout)));
        assert!(!client.connected());
    }

    #[test]
    fn test_client_tolerates_missing_target_until_send() {
        // The target may not be up yet (e.g. a GUI client started before
        // `umami` itself) -- construction must not fail just because of
        // that; only an actual `send` surfaces it, as a `Connection` error
        // rather than a panic on the still-empty `sock`.
        let mut client = Client::new(&unique_name("missing")).unwrap();
        assert!(!client.connected());
        assert!(matches!(client.send(&Command::Clear), Err(ClientError::Connection(_))));
    }

    #[test]
    fn test_reconnect_recovers_after_a_timeout() {
        let name = unique_name("recover");
        let server = bind_fake_server(&name);
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            // Silently drop the first request (the one that will time out),
            // then answer the second, made after send()'s own reconnect.
            let (_n, _peer) = server.recv_from(&mut buf).unwrap();
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("\"clear\""));
            server.send_to_addr(br#"{"result":"ok"}"#, &peer).unwrap();
        });

        let mut client = Client::with_timeout(&name, Duration::from_millis(50)).unwrap();
        assert!(matches!(client.send(&Command::Clear), Err(ClientError::Timeout)));
        assert!(!client.connected());
        assert!(matches!(client.send(&Command::Clear).unwrap(), CommandReply::Ok));
        assert!(client.connected());
    }
}
