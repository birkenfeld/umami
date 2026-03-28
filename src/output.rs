// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::{fs::File, io::BufWriter, path::PathBuf, time::Instant};
use anyhow::{anyhow, Context};
use itertools::Itertools;
use rkyv::{api::high::to_bytes_in, ser::writer::IoWriter};
use crate::lprintln;
use crate::channel::{Receiver, Sender};
use crate::config::OutputConfig;
use crate::error::UResult;
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
    print_every: usize,
    // Runtime
    started: Instant,
    ev_count: usize,
    debug_at: usize,
    last_ts: EventTime,
    out_of_order: usize,
}

impl Output for DiagOutput {
    fn from_config(config: toml::Table) -> UResult<Self> {
        let mask = config.get("event_mask").and_then(|v| v.as_str()).unwrap_or("");
        let mask: EventMask = bitflag_attr::parser::from_text(mask)
            .with_context(|| {
                format!("Invalid event_mask: {} - valid flags are {} and can be combined with '|'",
                        mask, EventMask::all().iter_names().map(|(name, _)| name).join(", "))
            })?;
        if mask.is_empty() {
            lprintln!(INFO, "Set an `event_mask` to print individual events in diag output");
        }

        Ok(DiagOutput {
            event_mask: mask,
            check_order: config.get("check_order").and_then(|v| v.as_bool()).unwrap_or(false),
            print_every: config.get("print_every").and_then(|v| v.as_integer()).unwrap_or(i64::MAX) as usize,
            started: Instant::now(),
            ev_count: 0,
            debug_at: 0,
            last_ts: EventTime::zero(),
            out_of_order: 0,
        })
    }

    // fn update_config(&mut self, _: toml::Table) -> UResult<()> {
    //     Ok(())
    // }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        self.started = Instant::now();
        self.ev_count = 0;
        self.debug_at = self.print_every;
        self.last_ts = EventTime::zero();
        self.out_of_order = 0;
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        let time = self.started.elapsed().as_secs_f32();
        let rate = if time > 0.0 { self.ev_count as f32 / time } else { 0.0 };
        lprintln!(INFO, "Ran for {:.3} s, total events: {}, rate: {} ev/s",
                  time, self.ev_count, rate);
        if self.out_of_order > 0 {
            lprintln!(INFO, "Total out of order: {}", self.out_of_order);
        }
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        self.ev_count += events.len();
        if self.ev_count >= self.debug_at {
            lprintln!(DEBUG, "Received {} events", self.debug_at);
            self.debug_at += self.print_every;
        }

        for ev in events {
            let ev_ts = ev.time;
            if self.check_order && ev_ts < self.last_ts {
                lprintln!(INFO, "Out of order event: last_ts={:?}, \
                                 ev_ts={:?}", self.last_ts, ev_ts);
                self.out_of_order += 1;
            }
            self.last_ts = ev_ts;
            let display = self.event_mask.contains(match ev.data {
                EventData::RawNeutron => EventMask::RAW_NEUTRON,
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
    id_buffer: Vec<u32>,
    offset_buffer: Vec<f64>,
}

impl HDF5EventsOutput {
    const BUFFER_SIZE: usize = 8192;

    fn map_to_index(x: u32, y: u32) -> u32 {
        1024 * y + x
    }

    fn write_chunk(&mut self) -> UResult<()> {
        if let Some(file) = &self.file {
            let event_id = file.dataset("event_id")
                               .context("Getting event id dataset")?;
            let event_offset = file.dataset("event_time_offset")
                                   .context("Getting event time offset dataset")?;
            let cur_size = event_id.size();
            let new_size = cur_size + self.id_buffer.len();
            event_id.resize(new_size).context("Resizing event id dataset")?;
            event_id.write_slice(&self.id_buffer, cur_size..new_size)
                    .context("Writing event id dataset")?;
            event_offset.resize(new_size).context("Resizing event time offset dataset")?;
            event_offset.write_slice(&self.offset_buffer, cur_size..new_size)
                        .context("Writing event time offset dataset")?;
        }
        self.id_buffer.clear();
        self.offset_buffer.clear();
        Ok(())
    }
}

impl Output for HDF5EventsOutput {
    fn from_config(_config: toml::Table) -> UResult<Self> where Self: Sized {
        Ok(HDF5EventsOutput {
            file: None,
            id_buffer: Vec::with_capacity(2 * Self::BUFFER_SIZE),
            offset_buffer: Vec::with_capacity(2 * Self::BUFFER_SIZE),
        })
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        let file = hdf5::File::create(format!("{}.h5", run))
            .with_context(|| format!("Creating HDF5 output file at {}.h5", run))?;
        let _ = file
            .new_dataset::<f64>()
            .shape(hdf5::Extent::resizable(0))
            .create("event_time_offset")
            .context("Creating time offset dataset")?;
        let _ = file
            .new_dataset::<u32>()
            .shape(hdf5::Extent::resizable(0))
            .create("event_id")
            .context("Creating event id dataset")?;
        self.file = Some(file);
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        self.write_chunk()?;
        self.file = None;
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        for event in events {
            match event.data {
                // TODO: zero timestamps handling (chopper?)
                EventData::Neutron { x, y, .. } => {
                    self.id_buffer.push(HDF5EventsOutput::map_to_index(x, y));
                    self.offset_buffer.push(event.rel_time.into());
                },
                _ => (),
            }
        }
        if self.id_buffer.len() >= Self::BUFFER_SIZE {
            self.write_chunk()?;
        }
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
