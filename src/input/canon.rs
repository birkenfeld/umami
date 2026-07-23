// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::{io, fmt, thread};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;
use anyhow::Context;
use byteorder::{ByteOrder, ReadBytesExt, WriteBytesExt, BE};
use crate::lprintln;
use crate::command::{Command, CommandReply, ModuleId};
use crate::config::{CanonConfig, SourceConfig};
use crate::error::{UError, UResult};
use crate::event::{Event, EventTime, EventType};
use crate::input::{ReplayFile, DumpHandler};
use super::{Source, Input, InputCommon};

pub struct CanonInput<S> {
    source: S,
    name: ModuleId,
    dump: DumpHandler,
    is_gate: bool,
    channel_ofs: u32,
    time_ofs: EventTime,
    buffer: Vec<u8>,
}

const MAX_EVENTS: usize = 1000;
const EVENT_SIZE: usize = 8;

// 01/01/2008 00:00:00 UTC, the epoch for Canon device time
const EPOCH: EventTime = EventTime::from_sec_nsec(1199145600, 0);

impl CanonInput<()> {
    pub fn start(config: CanonConfig, confdir: &Path, common: InputCommon) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => CanonInput::start_with_source(
                TcpStream::from_config(addr, confdir)?, config, common),
            SourceConfig::File(path) => CanonInput::start_with_source(
                ReplayFile::from_config(path, confdir)?, config, common),
        }
    }
}

impl<S: CanonSource> CanonInput<S> {
    pub fn start_with_source(source: S, config: CanonConfig, common: InputCommon) -> UResult<()> {
        let input = Self {
            source,
            name: common.name,
            dump: Default::default(),
            channel_ofs: config.channel_offset,
            is_gate: config.gatenet,
            time_ofs: EventTime::zero(),
            buffer: vec![0; EVENT_SIZE * MAX_EVENTS],
        };
        input.start_main_loop(common)?;
        Ok(())
    }
}

impl<S: CanonSource> Input for CanonInput<S> {
    fn description(&self) -> String {
        format!(
            "{} {} at {}",
            if self.is_gate { "GateNet" } else { "NeuNet" },
            self.name,
            self.source.description()
        )
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        if let Command::SetRawDump { enable, path } = cmd {
            self.dump.configure(enable, path)?;
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        self.dump.start(self.name, &run_id)?;
        self.source.rewind()?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.stop();
        Ok(())
    }

    fn reset(&mut self) -> UResult<()> {
        self.source.reset()?;
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        let nevents = self.source.request_events(&mut self.buffer)?;
        self.dump.write(&self.buffer[..nevents * EVENT_SIZE])?;

        // decode events
        let mut events = Vec::with_capacity(nevents);
        for i in 0..nevents {
            let cev = CanonEvent(BE::read_u64(&self.buffer[i * EVENT_SIZE..]));
            let event = match cev.evtype() {
                // We don't use the TriggerSync events
                CanonEvType::TriggerSync | CanonEvType::Trigger =>
                    continue,
                CanonEvType::DeviceTime => {
                    let time = EPOCH +
                        EventTime::from_ticks(1_000_000_000, cev.s()) +
                        EventTime::from_clock(32768, cev.ss()) +
                        EventTime::from_ticks(25, cev.us());
                    self.time_ofs = time;
                    Event::new(EventType::Tzero)
                        .with_channel(self.channel_ofs)
                        .with_abs_time_and_offset(self.time_ofs, EventTime::zero())
                },
                CanonEvType::DevTime32bit => {
                    let time = EPOCH +
                        EventTime::from_ticks(1_000_000_000, cev.s32()) +
                        EventTime::from_clock(32768, cev.ss32()) +
                        EventTime::from_ticks(25, cev.us32());
                    self.time_ofs = time;
                    Event::new(EventType::Tzero)
                        .with_channel(self.channel_ofs)
                        .with_abs_time_and_offset(self.time_ofs, EventTime::zero())
                },
                CanonEvType::Neutron => {
                    let t = EventTime::from_ticks(25, cev.t());
                    let pl = u32::from(cev.pl());
                    let pr = u32::from(cev.pr());
                    let chan = self.channel_ofs + u32::from(cev.p());
                    Event::new(EventType::Neutron)
                        .with_channel(chan)
                        .with_abs_time_and_offset(self.time_ofs, t)
                        .with_ampl(pl + pr)
                        .with_raw(pl, pr)
                },
                CanonEvType::Neutron14bit => {
                    let t = EventTime::from_ticks(25, cev.t());
                    let pl = u32::from(cev.pl14());
                    let pr = u32::from(cev.pr14());
                    let chan = self.channel_ofs + u32::from(cev.p14());
                    Event::new(EventType::Neutron)
                        .with_channel(chan)
                        .with_abs_time_and_offset(self.time_ofs, t)
                        .with_ampl(pl + pr)
                        .with_raw(pl, pr)
                },
                CanonEvType::External =>
                    continue,
                _ => {
                    lprintln!(WARN, [self.name] "Unknown event type: {}", cev);
                    continue;
                }
            };
            events.push(event);
        };

        Ok(events)
    }
}

pub trait CanonSource: Source {
    fn request_events(&mut self, buffer: &mut [u8]) -> UResult<usize>;
}

impl CanonSource for TcpStream {
    fn request_events(&mut self, buffer: &mut [u8]) -> UResult<usize> {
        // request event data (in 16-bit units)
        self.write_u64::<BE>(0xA300_0000_0000_0000 + (MAX_EVENTS as u64 * 4))
            .context("Requesting events")?;

        // read back the number of available 16-byte units
        let n = match self.read_u32::<BE>() {
            // if nothing available, sleep for a bit to avoid busy-waiting,
            // but still give the main loop a chance to check for commands
            Ok(0) => { thread::sleep(Duration::from_millis(100)); return Ok(0) }
            Ok(n) => n as usize / 4,
            // no answer? strange, but give it some time
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(0),
            // socket closed
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(UError::NoMoreData),
            Err(e) => Err(e).context("Reading number of events")?,
        };

        // read all event data (64 bits per)
        self.read_exact(&mut buffer[..n * EVENT_SIZE]).context("Reading events")?;
        Ok(n)
    }
}

impl CanonSource for ReplayFile {
    fn request_events(&mut self, buffer: &mut [u8]) -> UResult<usize> {
        // There are no headers in the replay file, so we just read as many events as
        // possible. `read` may return short of a full buffer even before EOF, so keep
        // reading until it is full; otherwise a short read could leave us with a
        // fractional trailing event and misalign every event read from then on.
        let mut total = 0;
        while total < buffer.len() {
            match io::Read::read(&mut self.file, &mut buffer[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => Err(e).context("Reading events from replay file")?,
            }
        }
        if total == 0 {
            return Err(UError::NoMoreData);
        }
        Ok(total / EVENT_SIZE)
    }
}


struct CanonEvent(u64);

#[derive(PartialEq, Eq, Debug)]
enum CanonEvType {
    TriggerSync,  // 0x51
    Neutron,      // 0x5a (neutron or external event)
    Trigger,      // 0x5b (T0 data)
    DeviceTime,   // 0x5c
    External,     // 0x5d (only from GateNET)
    Neutron14bit, // 0x5f
    DevTime32bit, // 0x6c
    Other(u8),
}

impl CanonEvent {
    fn evtype(&self) -> CanonEvType {
        match self.0 >> 56 {
            0x51 => CanonEvType::TriggerSync,
            0x5a => CanonEvType::Neutron,
            0x5b => CanonEvType::Trigger,
            0x5c => CanonEvType::DeviceTime,
            0x5d => CanonEvType::External,
            0x5f => CanonEvType::Neutron14bit,
            0x6c => CanonEvType::DevTime32bit,
            c =>    CanonEvType::Other(c as u8),
        }
    }

    // accessors for the different fields
    // evtype is not checked!

    /// Neutron and Neutron14bit: detection time after trigger pulse
    fn t(&self) -> u32 {
        ((self.0 >> 32) & 0xFFFFFF) as _
    }

    /// Neutron: PSD number (3 bit used)
    fn p(&self) -> u8 {
        ((self.0 >> 24) & 0x7) as _
    }

    /// Neutron: left pulse height
    fn pl(&self) -> u16 {
        ((self.0 >> 12) & 0xFFF) as _
    }

    /// Neutron: right pulse height
    fn pr(&self) -> u16 {
        (self.0 & 0xFFF) as _
    }

    /// Neutron14bit: PSD number (3 bit used)
    fn p14(&self) -> u8 {
        ((self.0 >> 28) & 0x7) as _
    }

    /// Neutron14bit: left pulse height
    fn pl14(&self) -> u16 {
        ((self.0 >> 14) & 0x3FFF) as _
    }

    /// Neutron14bit: right pulse height
    fn pr14(&self) -> u16 {
        (self.0 & 0x3FFF) as _
    }

    /// Trigger, TriggerSync and External: crate number
    fn c(&self) -> u8 {
        ((self.0 >> 48) & 0xFF) as _
    }

    /// Trigger, TriggerSync and External: module number
    fn m(&self) -> u8 {
        ((self.0 >> 40) & 0xFF) as _
    }

    /// Trigger, TriggerSync and External: ID of trigger signal (40bit)
    fn k(&self) -> u64 {
        (self.0 & 0xFFFFFFFFFF) as _
    }

    /// DeviceTime: seconds (30bit)
    fn s(&self) -> u32 {
        ((self.0 >> 26) & 0x3FFFFFFF) as _
    }

    /// DeviceTime: subseconds (in 1/32768 seconds)
    fn ss(&self) -> u16 {
        ((self.0 >> 11) & 0x7FFF) as _
    }

    /// DeviceTime: module clock (in 25 ns)
    fn us(&self) -> u16 {
        (self.0 & 0x7FF) as _
    }

    /// DevTime32: seconds
    fn s32(&self) -> u32 {
        ((self.0 >> 24) & 0xFFFFFFFF) as _
    }

    /// DevTime32: subseconds (in 1/32768 seconds)
    fn ss32(&self) -> u16 {
        ((self.0 >> 9) & 0x7FFF) as _
    }

    /// DevTime32: module clock (in 100 ns)
    fn us32(&self) -> u16 {
        (self.0 & 0x1FF) as _
    }

    // Not implemented: Self-oscillation mode for DevTime (subseconds
    // in 1/256 seconds)
}

/// Format a short description of the event.
impl fmt::Display for CanonEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.evtype() {
            CanonEvType::Other(_) =>
                write!(f, "??? V={:x}", self.0),
            CanonEvType::TriggerSync =>
                write!(f, "T0s C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            CanonEvType::Neutron =>
                write!(f, "N   T={:6x} P={} PL={:5} PR={:5}",
                       self.t(), self.p(), self.pl(), self.pr()),
            CanonEvType::Trigger =>
                write!(f, "T0  C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            CanonEvType::DeviceTime =>
                write!(f, "DT  S={:10} SS={:5} US={:4}",
                       self.s(), self.ss(), self.us()),
            CanonEvType::External =>
                write!(f, "EXT C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            CanonEvType::Neutron14bit =>
                write!(f, "N14 T={:6x} P={} PL={:5} PR={:5}",
                       self.t(), self.p14(), self.pl14(), self.pr14()),
            CanonEvType::DevTime32bit =>
                write!(f, "D32 S={:10} SS={:5} US={:4}",
                       self.s32(), self.ss32(), self.us32()),
        }
    }
}
