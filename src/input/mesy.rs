// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(unused)]

use std::fs::File;
use std::net::UdpSocket;
use anyhow::anyhow;
use crate::command::{Command, CommandReply};
use crate::config::{MesyConfig, SourceConfig};
use crate::error::UResult;
use crate::event::{ModuleId, Event};
use super::{Source, Input, InputPlumbing, UdpReader, NullCmdHandler};

pub struct MesyInput<S> {
    source: S,
    module: ModuleId,
}

impl MesyInput<()> {
    pub fn start(module: ModuleId, config: MesyConfig, plumbing: InputPlumbing) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => MesyInput::start_with_source(
                UdpReader::from_config(addr)?, module, config, plumbing),
            SourceConfig::File(path) => MesyInput::start_with_source(
                File::from_config(path)?, module, config, plumbing),
        }
    }
}

impl<S: Source> MesyInput<S> {
    fn start_with_source(source: S, module: ModuleId, config: MesyConfig,
                       plumbing: InputPlumbing) -> UResult<()> {
        let input = Self { source, module };
        input.start(module, plumbing);
        Ok(())
    }
}

impl<S: Source> Input for MesyInput<S> {
    type CmdHandler = NullCmdHandler;

    fn command_handler(&self, _: &InputPlumbing) -> Self::CmdHandler {
        NullCmdHandler(self.module)
    }

    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        unimplemented!()
    }
}
