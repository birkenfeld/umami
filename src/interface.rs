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
                    // TODO error handling
                    if let Ok(s) = str::from_utf8(&buf[..n])
                        && let Ok(cmd) = serde_json::from_str::<Command>(s)
                    {
                        ldebug!("Received command {:?}", cmd);
                        self.req_write.send(cmd).unwrap();
                        if let Ok(reply) = self.rep_read.recv() {
                            ldebug!("Sending reply {:?}", reply);
                            let r = serde_json::to_string(&reply).unwrap();
                            if let Err(e) = self.sock.send_to_addr(r.as_bytes(), &addr) {
                                lprintln!(ERROR, "Sending command reply: {e:#}");
                            }
                        }
                    }
                },
                Err(e) => {
                    lprintln!(ERROR, "Unix socket receive error: {e:#}");
                }
            }
        }
    }
}
