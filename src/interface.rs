// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::os::unix::net;
use anyhow::Context;
use uds::UnixDatagramExt;
use crate::{ldebug, lprintln};
use crate::channel::{Sender, Receiver};
use crate::command::{Command, CommandReply};
use crate::error::UResult;

// TODO combine with CommandHandler
pub struct UdsInterface {
    sock: net::UnixDatagram,
    req_write: Sender<Command>,
    rep_read: Receiver<CommandReply>,
}

impl UdsInterface {
    pub fn new(name: &str, req_write: Sender<Command>, rep_read: Receiver<CommandReply>) -> UResult<Self> {
        let addr = uds::UnixSocketAddr::from_abstract(name.as_bytes())
            .context("Creating abstract socket address")?;
        let sock = net::UnixDatagram::bind_unix_addr(&addr)
            .with_context(|| format!("Binding command listener to {}", addr))?;
        Ok(Self { sock, req_write, rep_read })
    }

    pub fn start(mut self) -> UResult<()> {
        std::thread::Builder::new()
            .name("UDS interface".into())
            .spawn(move || self.main())
            .context("Spawning interface thread")?;
        Ok(())
    }

    pub fn main(&mut self) {
        let mut buf = [0u8; 8192];
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, addr)) => {
                    let reply = self.handle_message(&buf[..n]);
                    let serialized = serde_json::to_string(&reply).expect("serializable");
                    if self.sock.send_to_addr(serialized.as_bytes(), &addr).is_err() {
                        lprintln!(ERROR, "Failed to send command reply to {:?}", addr);
                    }
                }
                Err(e) => {
                    lprintln!(ERROR, "Unix socket receive error: {e:#}");
                }
            }
        }
    }

    fn handle_message(&self, buf: &[u8]) -> CommandReply {
        match str::from_utf8(buf) {
            Err(_) => {
                CommandReply::new_error(None, format!("Invalid UTF-8 in telegram"))
            }
            Ok(s) => match serde_json::from_str::<Command>(s) {
                Err(e) => {
                    CommandReply::new_error(None, format!("Invalid JSON or invalid command: {e:#}"))
                }
                Ok(cmd) => {
                    ldebug!("Received command {:?}", cmd);
                    self.req_write.send(cmd).unwrap();
                    if let Ok(reply) = self.rep_read.recv() {
                        reply
                    } else {
                        CommandReply::new_error(None, "Failed to receive command reply".to_string())
                    }
                }
            },
        }
    }
}
