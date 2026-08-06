// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! A synthetic input backend used only in pipeline tests (see
//! [`crate::config::SpecificInputConfig::Test`]).

use serde::Deserialize;
use crate::command::{Command, CommandReply, ModuleId};
use crate::error::{UError, UResult};
use crate::event::{Event, EventHisto, EventType};
use crate::params::HasParams;
use super::{Input, InputCommon};

/// Config for this synthetic input backend: it generates one Neutron event
/// for every (x, y) cell in `0..nx` x `0..ny`, so tests can assert an exact
/// histogram.
#[derive(Debug, Deserialize)]
pub struct TestInputConfig {
    pub nx: u16,
    pub ny: u16,
}

#[derive(HasParams)]
#[params(kind = "input", type = "test")]
pub struct TestInput {
    name: ModuleId,
    // one Neutron event per (x, y) cell in 0..nx x 0..ny, re-issued on every run
    template: Vec<Event>,
    remaining: Option<Vec<Event>>,
}

pub fn start(config: TestInputConfig, common: InputCommon) -> UResult<()> {
    let mut template = Vec::with_capacity(config.nx as usize * config.ny as usize);
    for y in 0..config.ny {
        for x in 0..config.nx {
            let mut ev = Event::new(EventType::Neutron);
            ev.histo = EventHisto { x, y, t: 0, i: 0 };
            template.push(ev);
        }
    }
    let input = TestInput { name: common.name, template, remaining: None };
    input.start_main_loop(common)?;
    Ok(())
}

impl Input for TestInput {
    fn description(&self) -> String {
        format!("Test {}", self.name)
    }

    fn handle(&mut self, _cmd: Command) -> UResult<CommandReply> {
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, _run_id: String) -> UResult<()> {
        self.remaining = Some(self.template.clone());
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        Ok(())
    }

    fn reset(&mut self) -> UResult<()> {
        self.remaining = Some(self.template.clone());
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        match self.remaining.take() {
            Some(events) => Ok(events),
            None => Err(UError::NoMoreData),
        }
    }
}
