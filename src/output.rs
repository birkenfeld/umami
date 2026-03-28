// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::{anyhow, Context};
use crate::lprintln;
use crate::channel::{Receiver, Sender};
use crate::config::OutputConfig;
use crate::error::UResult;
use crate::event::Event;
use crate::pipeline::PipeItem;

mod diag;
mod file;
mod hdf5;

// TODO: file-based outputs: filename (needs to come from Command)?

pub struct OutputCommon {
    name: String,
    input: Receiver<PipeItem>,
    output: Option<Sender<PipeItem>>,
}

impl OutputCommon {
    pub fn new(
        name: String,
        input: Receiver<PipeItem>,
        output: Option<Sender<PipeItem>>,
    ) -> Self {
        Self { name, input, output }
    }
}


pub trait Output: Send {
    fn from_config(config: toml::Table) -> UResult<Self> where Self: Sized;
    // fn update_config(&mut self, _: toml::Table) -> UResult<()>;
    fn handle_events(&mut self, events: &[Event]) -> UResult<()>;
    fn handle_start_of_run(&mut self, run: &str) -> UResult<()>;
    fn handle_end_of_run(&mut self) -> UResult<()>;

    fn main_loop(mut self, common: OutputCommon)
    where Self: Sized
    {
        let mut current_run = String::new();
        while let Ok(item) = common.input.recv() {
            match &item {
                PipeItem::Events(events) => {
                    if let Err(e) = self.handle_events(events) {
                        lprintln!(ERROR, "Output {}: error handling events: {e:#}",
                                  common.name);
                    }
                },
                PipeItem::StartOfRun(run) => {
                    current_run = run.clone();
                    if let Err(e) = self.handle_start_of_run(run) {
                        lprintln!(ERROR, "Output {}: error handling start of run: {e:#}",
                                  common.name);
                    }
                },
                PipeItem::EndOfRun => {
                    if let Err(e) = self.handle_end_of_run() {
                        lprintln!(ERROR, "Output {}: error handling end of run: {e:#}",
                                  common.name);
                    }
                    lprintln!(INFO, "Output {}: finished with run {:?}", common.name, current_run);
                },
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
        lprintln!(INFO, "Initialized output {}", common.name);
        std::thread::Builder::new()
            .name(format!("Output {}", common.name))
            .spawn(move || self.main_loop(common))
            .context("Spawning output thread")?;
        Ok(())
    }
}

/// Absorb all events without action.
pub struct NullOutput;

impl Output for NullOutput {
    fn from_config(_: toml::Table) -> UResult<Self> {
        Ok(NullOutput)
    }

    // fn update_config(&mut self, _: toml::Table) -> UResult<()> {
    //     Ok(())
    // }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        Ok(())
    }
    fn handle_end_of_run(&mut self) -> UResult<()> {
        Ok(())
    }

    fn handle_events(&mut self, _events: &[Event]) -> UResult<()> {
        Ok(())
    }
}

// Broadcast over udp/uds
// pub struct UDPOutput;
// pub struct UDSOutput;

pub fn start(config: OutputConfig, common: OutputCommon) -> UResult<()> {
    match config.r#type.as_str() {
        "none" => Ok(NullOutput::from_config(config.config)?.start(common)?),
        "diag" => Ok(diag::DiagOutput::from_config(config.config)?.start(common)?),
        "hdf5" => Ok(hdf5::HDF5EventsOutput::from_config(config.config)?.start(common)?),
        "file" => Ok(file::FileOutput::from_config(config.config)?.start(common)?),
        _ => Err(anyhow!("Unknown output type: {}", config.r#type).into()),
    }
}
