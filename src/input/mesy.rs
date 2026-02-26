// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(unused)]

use std::fs::File;
use std::net::UdpSocket;
use anyhow::anyhow;
use crate::config::{MesyConfig, SourceConfig};
use crate::error::UResult;
use crate::event::{ModuleId, Event};
use super::{Source, Input, InputChannels, UdpReader};

pub struct MesyInput<S> {
    source: S,
    module: ModuleId,
    channels: InputChannels,
}

impl MesyInput<()> {
    pub fn init(module: ModuleId, config: MesyConfig, channels: InputChannels) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => MesyInput::init_with_source(UdpReader::from_config(addr)?,
                                                                  module, config, channels),
            SourceConfig::File(path) => MesyInput::init_with_source(File::from_config(path)?,
                                                                    module, config, channels),
        }
    }
}

impl<S: Source> MesyInput<S> {
    fn init_with_source(source: S, module: ModuleId, config: MesyConfig,
                        channels: InputChannels) -> UResult<()> {
        let input = Self { source, module, channels };
        input.start_event_thread();
        Ok(())
    }
}

impl<S: Source> Input for MesyInput<S> {
    fn channels(&self) -> &InputChannels {
        &self.channels
    }


    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        unimplemented!()
    }
}
