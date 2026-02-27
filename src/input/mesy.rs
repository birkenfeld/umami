// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(unused)]

use std::fs::File;
use std::net::UdpSocket;
use anyhow::anyhow;
use crate::config::{MesyConfig, SourceConfig};
use crate::error::UResult;
use crate::event::{ModuleId, Event};
use super::{Source, Input, InputPlumbing, UdpReader};

pub struct MesyInput<S> {
    source: S,
    module: ModuleId,
    plumbing: InputPlumbing,
}

impl MesyInput<()> {
    pub fn init(module: ModuleId, config: MesyConfig, plumbing: InputPlumbing) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => MesyInput::init_with_source(UdpReader::from_config(addr)?,
                                                                  module, config, plumbing),
            SourceConfig::File(path) => MesyInput::init_with_source(File::from_config(path)?,
                                                                    module, config, plumbing),
        }
    }
}

impl<S: Source> MesyInput<S> {
    fn init_with_source(source: S, module: ModuleId, config: MesyConfig,
                        plumbing: InputPlumbing) -> UResult<()> {
        let input = Self { source, module, plumbing };
        input.start_event_thread();
        Ok(())
    }
}

impl<S: Source> Input for MesyInput<S> {
    fn plumbing(&self) -> &InputPlumbing {
        &self.plumbing
    }


    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        unimplemented!()
        // events = self.plumbing.recipe.process(events);
    }
}
