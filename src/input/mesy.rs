// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::VecDeque;
use std::net::UdpSocket;
use anyhow::anyhow;
use crate::error::{UError, UResult};
use crate::event::{ModuleId, Event};
use crate::util::resolve;

pub struct MesyInput {
    module: ModuleId,
    socket: UdpSocket,
    buffer: VecDeque<Event>,
}

impl crate::input::Input for MesyInput {
    fn description(&self) -> String {
        format!(
            "MCPD module {} at {}",
            self.module.0,
            self.socket.peer_addr().map(|x| x.to_string()).unwrap_or("?".into()),
        )
    }

    fn read_event(&mut self) -> UResult<Event> {
        if let Some(ev) = self.buffer.pop_front() {
            return Ok(ev);
        }

        return Err(anyhow!("nope").into());
    }
}

impl MesyInput {
    pub fn new(module: ModuleId, local: String, addr: String) -> UResult<Self> {
        let socket = UdpSocket::bind(resolve(&local)?).map_err(UError::SourceInit)?;
        socket.connect(resolve(&addr)?).map_err(UError::SourceInit)?;
        Ok(Self {
            module,
            socket,
            buffer: VecDeque::with_capacity(32),
        })
    }
}
