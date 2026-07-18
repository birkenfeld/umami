// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use rkyv::{Archive, Serialize, Deserialize};

/// Timestamp of the event in nanoseconds.
///
/// Should be absolute (relative to UNIX epoch) if possible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[derive(Archive, Serialize, Deserialize)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EventTime(i64);

impl Display for EventTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{:.9}s", self.0 as f64 / 1_000_000_000.0)
    }
}

impl EventTime {
    pub const MAX: Self = Self(i64::MAX);

    pub const fn zero() -> Self {
        Self(0)
    }

    pub const fn from_sec_nsec(sec: u32, nsec: u32) -> Self {
        Self(sec as i64 * 1_000_000_000 + nsec as i64)
    }

    pub const fn from_floating_sec(sec: f64) -> Self {
        Self((sec * 1_000_000_000.0) as i64)
    }

    pub fn from_ticks<T>(ns_per: i64, ticks: T) -> Self where i64: From<T> {
        Self(i64::from(ticks) * ns_per)
    }

    pub fn from_clock<T>(freq: i64, ticks: T) -> Self where i64: From<T> {
        Self(i64::from(ticks) * 1_000_000_000 / freq)
    }
}

impl From<EventTime> for f64 {
    fn from(value: EventTime) -> Self {
        value.0 as f64 / 1_000_000_000.0
    }
}

impl std::ops::Add for EventTime {
    type Output = Self;

    fn add(self, other: EventTime) -> Self {
        Self(self.0 + other.0)
    }
}

impl std::ops::Sub for EventTime {
    type Output = Self;

    fn sub(self, other: Self) -> EventTime {
        EventTime(self.0 - other.0)
    }
}

/// Input channel of the event - a tube or pixel ID for neutrons, or a signal
/// for edges or other kinds of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct ChannelId(pub u32);

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Archive, Serialize, Deserialize)]
pub enum EventData {
    /// Neutron event.
    Neutron = 0x01,
    /// Monitor count.
    Monitor = 0x02,
    /// Signal edge without further meaning.
    Edge { up: bool } = 0x10,
    /// Gate signal.
    Gate { up: bool } = 0x11,
    /// T-zero signal (usually chopper).
    Tzero = 0x12,
    /// Additional signal.
    AuxSignal { num: u8 } = 0x13,
    /// Heartbeat from hardware.
    Heartbeat = 0x80,
    /// Sorted-out event.
    Void = 0xFF,
}

impl Eq for EventData {}

#[bitflag_attr::bitflag(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub enum EventFlags {
    None = 0,
    HasRelTime = 1,
    Fake = 0x1000,
}

impl Display for EventFlags {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        if self.is_empty() {
            return write!(f, "-");
        }
        let mut first = true;
        for flag in self {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            match flag {
                Self::HasRelTime => write!(f, "RT")?,
                Self::Fake => write!(f, "F")?,
                _ => unreachable!(),
            }
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct Event {
    // Do not change the structure, the serialization format depends on it.
    pub time: EventTime,
    pub rel_time: EventTime,  // zeroed until determined
    pub channel: ChannelId,
    pub ampl: u32,
    // histogram coordinates
    pub x: u32,
    pub y: u32,
    pub t: u32,
    pub i: u32,
    pub flags: EventFlags,
    pub data: EventData,
}

impl Event {
    pub fn new(time: EventTime, rel_time: EventTime, channel: ChannelId,
               flags: EventFlags, data: EventData, ampl: u32) -> Self {
        Self { time, rel_time, flags, channel, data, x: 0, y: 0, t: 0, i: 0, ampl }
    }

    pub fn dump(&self) -> DumpEvent<'_> {
        DumpEvent(self)
    }
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Event(time={:.9}, rel_time={:.9}, flags={:#x}, channel={}, data={:?})",
               self.time.0 as f64 / 1_000_000_000.0,
               self.rel_time.0 as f64 / 1_000_000_000.0,
               self.flags.0, self.channel.0, self.data)
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time.0.cmp(&other.time.0)
    }
}

pub struct DumpEvent<'a>(&'a Event);

impl Display for DumpEvent<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let ev = self.0;
        write!(f, "{:.9} / {:.9} [{}] C{:3} ",
               ev.time.0 as f64 / 1_000_000_000.0,
               ev.rel_time.0 as f64 / 1_000_000_000.0,
               ev.flags, ev.channel.0)?;
        match ev.data {
            EventData::Neutron =>
                write!(f, "Neutron"),
            EventData::Edge { up } =>
                write!(f, "Edge      {}", if up { "up" } else { "down" }),
            EventData::Heartbeat =>
                write!(f, "Heartbeat"),
            EventData::Monitor =>
                write!(f, "Monitor"),
            EventData::Tzero =>
                write!(f, "T-zero"),
            EventData::Gate { up } =>
                write!(f, "Gate      {}", if up { "up" } else { "down" }),
            EventData::AuxSignal { num } =>
                write!(f, "AuxSignal {num}"),
            EventData::Void =>
                write!(f, "Void"),
        }
    }
}

#[test]
fn test_event_size() {
    use std::mem::size_of;
    assert_eq!(size_of::<Event>(), 48);
}
