// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context};

use crate::channel::{Receiver, Sender};
use crate::config::OutputConfig;
use crate::error::UResult;
use crate::event::{Event, EventData};
use crate::pipeline::PipeItem;


// TODO:
// - file-based outputs: filename?
// - always the NullOutput at the end or check for optional sender?
// - let implementor match on pipeitem?

pub struct OutputCommon {
    input: Receiver<PipeItem>,
    output: Option<Sender<PipeItem>>,
}

impl OutputCommon {
    pub fn new(input: Receiver<PipeItem>, output: Option<Sender<PipeItem>>) -> Self {
        Self { input, output }
    }
}


pub trait Output: Send {
    fn from_config(_: toml::Table, _: &BTreeMap<String, OutputConfig>) -> UResult<Self>
        where Self: Sized;
    fn update_config(&mut self, _: toml::Table) -> UResult<()>;
    fn handle_events(&mut self, events: &[Event]) -> UResult<()>;
    fn handle_start_of_run(&mut self, run: &str) -> UResult<()>;
    fn handle_end_of_run(&mut self, run: String) -> UResult<()>;
    fn main_loop(mut self, common: OutputCommon)
    where Self: Sized
    {
        while let Ok(item) = common.input.recv() {
            match &item {
                PipeItem::Events(events) => {
                    let _ = self.handle_events(&events);
                },
                PipeItem::StartOfRun(run) => {
                    let _ = self.handle_start_of_run(run);
                },
                PipeItem::EndOfRun => {},
                PipeItem::Clear => {},
                _ => {},
            }
            if let Some(sender) = &common.output {
                let _ = sender.send(item);
            }
        }
    }
    fn start(self, common: OutputCommon) -> UResult<()>
    where Self: Sized + 'static
    {
        std::thread::Builder::new()
            .name("Output".into())
            .spawn(move || self.main_loop(common))
            .context("Spawning output thread")?;
        Ok(())
    }
}

/// Absorb all events without action.
pub struct NullOutput;

impl Output for NullOutput {
    fn from_config(_: toml::Table, _: &BTreeMap<String, OutputConfig>) -> UResult<Self> {
        Ok(NullOutput)
    }

    fn update_config(&mut self, _: toml::Table) -> UResult<()> {
        Ok(())
    }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        Ok(())
    }
    fn handle_end_of_run(&mut self, _run: String) -> UResult<()> {
        Ok(())
    }

    fn handle_events(&mut self, _events: &[Event]) -> UResult<()> {
        Ok(())
    }
}

pub struct DiagOutput;

// TODO move diag from postproc here
impl Output for DiagOutput {
    fn from_config(_: toml::Table, _: &BTreeMap<String, OutputConfig>) -> UResult<Self> {
        Ok(DiagOutput)
    }

    fn update_config(&mut self, _: toml::Table) -> UResult<()> {
        Ok(())
    }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        Ok(())
    }
    fn handle_end_of_run(&mut self, _run: String) -> UResult<()> {
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        for ev in events {
            if let EventData::Neutron { x, y, t } = &ev.data {
                println!("{} Neutron at {:3}, {:3}, {:3} ", ev.time, x, y, t);
            }
        }
        Ok(())
    }
}
    // std::thread::Builder::new()
    //     .name("Output".into())
    //     .spawn(move || while output_recv.recv().is_ok() {})
    //     .context("Spawning output thread")?;

// Broadcast over udp/uds
pub struct UDPOutput;
pub struct UDSOutput;


pub struct FileOutput;
pub struct HDF5EventsOutput;


pub enum OutputKind {
    Null(Box<NullOutput>),
    Diag(Box<DiagOutput>),
    // HDF5Events(Box<HDF5EventsOutput>),
}

impl OutputKind {
    pub fn start(self, common: OutputCommon) -> UResult<()> {
        match self {
            Self::Null(output) => output.start(common),
            Self::Diag(output) => output.start(common),
            // Self::HDF5Events(output) => output.start(common),
        }
    }
}

pub fn from_config(config: &OutputConfig) -> UResult<OutputKind> {
    match config.r#type.as_str() {
        "none" => Ok(OutputKind::Null(Box::new(NullOutput))),
        "diag" => Ok(OutputKind::Diag(Box::new(DiagOutput))),
        _ => Err(anyhow!("Unknown output type: {}", config.r#type).into()),
    }
}
