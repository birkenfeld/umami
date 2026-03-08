// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(unused)]

use std::net::UdpSocket;
use anyhow::anyhow;
use crate::command::{Command, CommandReply};
use crate::config::{MesyConfig, SourceConfig};
use crate::error::UResult;
use crate::event::{ModuleId, Event};
use crate::input::ReplayFile;
use super::{Source, Input, InputCommon, UdpReader};

pub struct MesyInput<S> {
    source: S,
    module: ModuleId,
}

impl MesyInput<()> {
    pub fn start(config: MesyConfig, common: InputCommon) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => MesyInput::start_with_source(
                UdpReader::from_config(addr)?, config, common),
            SourceConfig::File(path) => MesyInput::start_with_source(
                ReplayFile::from_config(path)?, config, common),
        }
    }
}

impl<S: Source> MesyInput<S> {
    fn start_with_source(source: S, config: MesyConfig, common: InputCommon) -> UResult<()> {
        let input = Self {
            source,
            module: common.module,
        };
        input.start_main_loop(common)?;
        Ok(())
    }
}

impl<S: Source> Input for MesyInput<S> {
    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn handle(&mut self, _cmd: Command) -> UResult<CommandReply> {
        Ok(CommandReply::Ok)
    }

    fn start(&mut self) -> UResult<()> {
        self.source.reset()?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        unimplemented!()
    }
}
