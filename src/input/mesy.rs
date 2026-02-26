// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(unused)]

use std::collections::VecDeque;
use std::net::UdpSocket;
use anyhow::anyhow;
use crate::config::MesyConfig;
use crate::error::UResult;
use crate::event::{ModuleId, Event};

pub struct MesyInput {
    module: ModuleId,
    socket: UdpSocket,
    buffer: VecDeque<Event>,
}

impl MesyInput {
    pub fn init(module: ModuleId, config: MesyConfig, channels: super::InputChannels) -> UResult<()> {
        unimplemented!()
    }

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
