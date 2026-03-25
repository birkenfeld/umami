// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::{fs::File, io::BufWriter, path::PathBuf};
use anyhow::{anyhow, Context};
use hdf5;
use itertools::Itertools;
use rkyv::{api::high::to_bytes_in, ser::writer::IoWriter};
use crate::lprintln;
use crate::channel::{Receiver, Sender};
use crate::config::OutputConfig;
use crate::error::{UError, UResult};
use crate::event::{Event, EventData, EventTime};
use crate::pipeline::PipeItem;


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
        lprintln!(INFO, "Initialized output {}", common.name);
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


#[bitflag_attr::bitflag(u16)]
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventMask {
    RAW_NEUTRON = 0x1,
    RAW_EDGE = 0x2,
    RAW_ANALOG = 0x4,
    RAW_DIGITAL = 0x8,
    RAW_DATA = 0x10,
    HEARTBEAT = 0x20,

    NEUTRON = 0x40,
    MONITOR = 0x80,
    TZERO = 0x100,
    GATE = 0x200,
    AUX = 0x400,
    VOID = 0x800,

    ALL_RAW = 0x3F,
    ALL_COOKED = 0x7C0,
    ALL = 0xFFF,
}


/// Output selected events, and count out-of-order events.
pub struct DiagOutput {
    // Configuration
    event_mask: EventMask,
    check_order: bool,
    // Runtime
    last_ts: EventTime,
    out_of_order: usize,
}

impl Output for DiagOutput {
    fn from_config(config: toml::Table) -> UResult<Self> {
        let mask = config.get("event_mask").and_then(|v| v.as_str()).unwrap_or("ALL_COOKED");
        let mask = bitflag_attr::parser::from_text(mask)
            .with_context(|| {
                format!("Invalid event_mask: {} - valid flags are {} and can be combined with '|'",
                        mask, EventMask::all().iter_names().map(|(name, _)| name).join(", "))
            })?;

        Ok(DiagOutput {
            event_mask: mask,
            check_order: config.get("check_order").and_then(|v| v.as_bool()).unwrap_or(false),
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
            if self.check_order && ev_ts < self.last_ts {
                lprintln!(INFO, "Out of order event: last_ts={:?}, \
                                 ev_ts={:?}", self.last_ts, ev_ts);
                self.out_of_order += 1;
            }
            self.last_ts = ev_ts;
            let display = self.event_mask.contains(match ev.data {
                EventData::RawNeutron { .. } => EventMask::RAW_NEUTRON,
                EventData::RawEdge { .. } => EventMask::RAW_EDGE,
                EventData::RawAnalog1 { .. } => EventMask::RAW_ANALOG,
                EventData::RawAnalog2 { .. } => EventMask::RAW_ANALOG,
                EventData::RawDigital { .. } => EventMask::RAW_DIGITAL,
                EventData::RawData { .. } => EventMask::RAW_DATA,
                EventData::Heartbeat => EventMask::HEARTBEAT,
                EventData::Neutron { .. } => EventMask::NEUTRON,
                EventData::Monitor { .. } => EventMask::MONITOR,
                EventData::Tzero => EventMask::TZERO,
                EventData::Gate { .. } => EventMask::GATE,
                EventData::AuxSignal { .. } => EventMask::AUX,
                EventData::Void => EventMask::VOID,
            });
            if display {
                lprintln!(INFO, "{}", ev.dump());
            }
        }
        Ok(())
    }
}

// TODO:
// - behind feature?
// - filename template
/// Output for a HDF5-File containing events following the NXevent_data format.
///
///  Currently the following fields are not supported:
///  "event_time_zero"
///  "event_index"
///  "cue_timestamp_zero"
///  "cue_index"
///  "pulse_height"
///
pub struct HDF5EventsOutput {
    file: Option<hdf5::File>,
}

impl HDF5EventsOutput {
    fn map_to_index(x: u32, y: u32) -> u32 {
        x + 100 * y
    }
}

impl Output for HDF5EventsOutput {
    fn from_config(_config: toml::Table) -> UResult<Self> where Self: Sized {
        Ok(HDF5EventsOutput { file: None })
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        if let Some(file) = &self.file {
            let event_id = file.dataset("event_id").map_err(|e| UError::Other(e.into()))?;
            let event_offset = file.dataset("event_time_offset").map_err(|e| UError::Other(e.into()))?;
            let mut ids = Vec::with_capacity(events.len());
            let mut offsets: Vec<f64> = Vec::with_capacity(events.len());
            for event in events {
                match event.data {
                    // TODO: zero timestamps handling (chopper?)
                    EventData::Neutron { x, y, t } => {
                        ids.push(HDF5EventsOutput::map_to_index(x, y));
                        // TODO: What does t mean vs event.time?
                        offsets.push(t.into());
                    },
                    _ => (),
                }
            }
            event_id.resize(event_id.size() + ids.len()).map_err(|e| UError::Other(e.into()))?;
            event_id.write_slice(&ids, 0..ids.len()).map_err(|e| UError::Other(e.into()))?;
            event_offset.resize(event_offset.size() + offsets.len()).map_err(|e| UError::Other(e.into()))?;
            event_offset.write_slice(&offsets, 0..offsets.len()).map_err(|e| UError::Other(e.into()))?;
        }
        Ok(())
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        self.file = Some(hdf5::File::create(format!("{}.h5", run)).map_err(|e| UError::Other(e.into()))?);
        let file = self.file.as_ref().unwrap();
        // events
        let builder = file.new_dataset::<f64>();
        let _ = builder.shape(hdf5::Extent::resizable(0)).create("event_time_offset").map_err(|e| UError::Other(e.into()))?;
        let builder = file.new_dataset::<u32>();
        let _ = builder.shape(hdf5::Extent::resizable(0)).create("event_id").map_err(|e| UError::Other(e.into()))?;
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        // let file = self.file.as_ref();
        // drop(file);
        self.file = None;
        Ok(())
    }
}

// Broadcast over udp/uds
// pub struct UDPOutput;
// pub struct UDSOutput;

pub struct FileOutput {
    // Configuration
    dir: PathBuf,
    filename: Option<String>,
    // Runtime
    writer: Option<IoWriter<BufWriter<File>>>,
}

impl Output for FileOutput {
    fn from_config(config: toml::Table) -> UResult<Self> where Self: Sized {
        let dir = config.get("dir")
            .ok_or_else(|| anyhow!("Missing 'dir' in file output config"))?
            .as_str()
            .ok_or_else(|| anyhow!("'dir' in file output config must be a string"))?;
        Ok(FileOutput { writer: None, filename: None, dir: PathBuf::from(dir) })
    }

    // TODO: config api to set the filename

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        if let Some(filename) = &self.filename {
            let path = self.dir.join(filename);
            let file = File::create(&path)
                .with_context(|| format!("Creating output file {}", path.display()))?;
            self.writer = Some(IoWriter::new(BufWriter::with_capacity(1 << 20, file)));
        } else {
            self.writer = None;
        }
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        self.writer = None;
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        if let Some(mut writer) = self.writer.as_mut() {
            for event in events {
                to_bytes_in::<_, rkyv::rancor::Failure>(event, &mut writer)
                    .context("Serializing event for file output")?;
            }
        }
        Ok(())
    }
}


pub fn start(config: OutputConfig, common: OutputCommon) -> UResult<()> {
    match config.r#type.as_str() {
        "none" => Ok(NullOutput::from_config(config.config)?.start(common)?),
        "diag" => Ok(DiagOutput::from_config(config.config)?.start(common)?),
        "hdf5" => Ok(HDF5EventsOutput::from_config(config.config)?.start(common)?),
        "file" => Ok(FileOutput::from_config(config.config)?.start(common)?),
        _ => Err(anyhow!("Unknown output type: {}", config.r#type).into()),
    }
}
