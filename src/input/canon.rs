// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::VecDeque;
use std::{fmt, thread};
use std::io::{self, Read};
use std::net::TcpStream;
use std::time::Duration;
use anyhow::anyhow;
use byteorder::{WriteBytesExt, ReadBytesExt, BE};
use crate::error::{UError, UResult};
use crate::event::{ModuleId, InputId, Event, EventTime, EventFlags, EventData};
use crate::util::resolve;

pub struct CanonInput {
    module: ModuleId,
    socket: TcpStream,
    buffer: VecDeque<Event>,
    is_gate: bool,
    time_ofs: EventTime,
}

// 01/01/2008 00:00:00 UTC, the epoch for Canon device time
const EPOCH: u64 = 1199145600 * 1_000_000_000;

impl crate::input::Input for CanonInput {
    fn description(&self) -> String {
        format!(
            "{} module {} at {}",
            if self.is_gate { "GateNet" } else { "NeuNet" },
            self.module.0,
            self.socket.peer_addr().map(|x| x.to_string()).unwrap_or("?".into()),
        )
    }

    fn read_event(&mut self) -> UResult<Event> {
        if let Some(ev) = self.buffer.pop_front() {
            return Ok(ev);
        }

        let n = loop {
            // request up to 0xFFFF 16-byte units of event data
            self.socket.write_u64::<BE>(0xA300_0000_0000_FFFF)?;

            // read back the number of available 16-byte units
            let n = self.socket.read_u32::<BE>().unwrap();
            if n > 0 {
                break n;
            }
            thread::sleep(Duration::from_millis(100));
        };

        // read events (4 16-bit units per event)
        for _ in 0..n/4 {
            let cev = CanonEvent::read(&mut self.socket).unwrap();
            let event = match cev.evtype() {
                // We don't use the TriggerSync events
                EventType::TriggerSync | EventType::Trigger =>
                    continue,
                EventType::DeviceTime => {
                    let time = EPOCH + cev.s() as u64 * 1_000_000_000 +
                        cev.ss() as u64 * 1_000_000_000 / 32768 +
                        cev.us() as u64 * 25;
                    self.time_ofs = EventTime::from_nsec(time);
                    Event::new(
                        self.time_ofs,
                        self.module,
                        InputId(0),
                        EventFlags::None,
                        EventData::Tzero,
                    )
                },
                EventType::DevTime32bit => {
                    let time = EPOCH + cev.s32() as u64 * 1_000_000_000 +
                        cev.ss32() as u64 * 1_000_000_000 / 32768 +
                        cev.us32() as u64 * 100;
                    self.time_ofs = EventTime::from_nsec(time);
                    Event::new(
                        self.time_ofs,
                        self.module,
                        InputId(0),
                        EventFlags::None,
                        EventData::Tzero,
                    )
                },
                EventType::Neutron => {
                    Event::new(
                        self.time_ofs + EventTime::from_nsec(cev.t() as u64 * 25),
                        self.module,
                        InputId(cev.p() as u16),
                        EventFlags::None,
                        EventData::Digital { value1: cev.pl() as u32, value2: cev.pr() as u32,
                                             value3: 0 },
                    )
                },
                EventType::Neutron14bit => {
                    Event::new(
                        self.time_ofs + EventTime::from_nsec(cev.t() as u64 * 25),
                        self.module,
                        InputId(cev.p14() as u16),
                        EventFlags::None,
                        EventData::Digital { value1: cev.pl14() as u32, value2: cev.pr14() as u32,
                                             value3: 0 },
                    )
                },
                EventType::External =>
                    continue,
                // TODO log instead?
                _ => return Err(anyhow!("Unsupported packet type {:?}", cev.evtype()).into()),
            };
            self.buffer.push_back(event);
        };

        // Since some events are skipped, the buffer may still be empty here.
        // In that case, we just wait for the next batch of events.
        match self.buffer.pop_front() {
            Some(ev) => Ok(ev),
            None => self.read_event(),
        }
    }
}

impl CanonInput {
    pub fn new(module: ModuleId, addr: &str, gate: bool) -> UResult<Self> {
        let socket = TcpStream::connect(resolve(addr)?)
            .map_err(UError::SourceInit)?;
        Ok(Self {
            module,
            socket,
            is_gate: gate,
            buffer: VecDeque::with_capacity(32),
            time_ofs: EventTime::from_nsec(0),
        })
    }
}



struct CanonEvent(u64);

#[derive(PartialEq, Eq, Debug)]
enum EventType {
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
    fn read<R: Read>(read: &mut R) -> io::Result<Self> {
        read.read_u64::<BE>().map(Self)
    }

    fn evtype(&self) -> EventType {
        match self.0 >> 56 {
            0x51 => EventType::TriggerSync,
            0x5a => EventType::Neutron,
            0x5b => EventType::Trigger,
            0x5c => EventType::DeviceTime,
            0x5d => EventType::External,
            0x5f => EventType::Neutron14bit,
            0x6c => EventType::DevTime32bit,
            c => return EventType::Other(c as u8),
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
        ((self.0 >> 24) & 0xFF) as _
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
        ((self.0 >> 28) & 0xF) as _
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
            EventType::Other(_) =>
                write!(f, "??? V={:x}", self.0),
            EventType::TriggerSync =>
                write!(f, "T0s C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            EventType::Neutron =>
                write!(f, "N   T={:6x} P={} PL={:5} PR={:5}",
                       self.t(), self.p(), self.pl(), self.pr()),
            EventType::Trigger =>
                write!(f, "T0  C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            EventType::DeviceTime =>
                write!(f, "DT  S={:10} SS={:5} US={:4}",
                       self.s(), self.ss(), self.us()),
            EventType::External =>
                write!(f, "EXT C={:3} M={:3} K={:10x}",
                       self.c(), self.m(), self.k()),
            EventType::Neutron14bit =>
                write!(f, "N14 T={:6x} P={} PL={:5} PR={:5}",
                       self.t(), self.p14(), self.pl14(), self.pr14()),
            EventType::DevTime32bit =>
                write!(f, "D32 S={:10} SS={:5} US={:4}",
                       self.s32(), self.ss32(), self.us32()),
        }
    }
}
