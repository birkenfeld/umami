// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::{anyhow, Context};
use crate::lprintln;
use crate::channel::{Receiver, Sender};
use crate::config::OutputConfig;
use crate::error::UResult;
use crate::event::{Event, EventTime};
use crate::pipeline::PipeItem;


// TODO:
// - file-based outputs: filename?
// - let implementor match on pipeitem?
// - Result returns needed? Should Err skip the rest of outputs?

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
        while let Ok(item) = common.input.recv() {
            match &item {
                PipeItem::Events(events) => {
                    if let Err(e) = self.handle_events(&events) {
                        lprintln!(ERROR, "Output {}: error handling events: {e:#}",
                                  common.name);
                    }
                },
                PipeItem::StartOfRun(run) => {
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

pub struct DiagOutput {
    every_event: bool,
    last_ts: EventTime,
    out_of_order: usize,
}

impl Output for DiagOutput {
    fn from_config(config: toml::Table) -> UResult<Self> {
        Ok(DiagOutput {
            every_event: config.get("every_event").and_then(|v| v.as_bool()).unwrap_or(false),
            last_ts: EventTime::zero(),
            out_of_order: 0,
        })
    }

    // fn update_config(&mut self, _: toml::Table) -> UResult<()> {
    //     Ok(())
    // }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        self.out_of_order = 0;
        self.last_ts = EventTime::zero();
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        if self.out_of_order > 0 {
            lprintln!(INFO, "Total out of order: {}", self.out_of_order);
        }
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        for ev in events {
            let ev_ts = ev.time;
            if ev_ts < self.last_ts {
                // lprintln!(INFO, "Out of order event: last_ts={:?}, \
                           // ev_ts={:?}", self.last_ts, ev_ts);
                self.out_of_order += 1;
            }
            self.last_ts = ev_ts;
            if self.every_event {
                lprintln!(INFO, "{}", ev.dump());
            }
        }
        Ok(())
    }
}

// Broadcast over udp/uds
// pub struct UDPOutput;
// pub struct UDSOutput;
// pub struct FileOutput;
// pub struct HDF5EventsOutput;


pub fn start(config: OutputConfig, common: OutputCommon) -> UResult<()> {
    match config.r#type.as_str() {
        "none" => Ok(NullOutput::from_config(config.config)?.start(common)?),
        "diag" => Ok(DiagOutput::from_config(config.config)?.start(common)?),
        _ => Err(anyhow!("Unknown output type: {}", config.r#type).into()),
    }
}
