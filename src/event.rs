// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use rkyv::{Archive, Serialize, Deserialize};

/// Timestamp of the event in nanoseconds.
///
/// Should be absolute (relative to UNIX epoch) if possible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
#[derive(Archive, Serialize, Deserialize)]
pub struct EventTime(i64);

impl Display for EventTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{:.9}s", self.0 as f64 / 1_000_000_000.0)
    }
}

impl EventTime {
    pub const fn zero() -> Self {
        Self(0)
    }

    pub const fn from_nsec(nsec: i64) -> Self {
        Self(nsec)
    }

    pub const fn from_sec_nsec(sec: u32, nsec: u32) -> Self {
        Self(sec as i64 * 1_000_000_000 + nsec as i64)
    }

    pub fn from_clock<T>(freq: i64, ticks: T) -> Self where i64: From<T> {
        Self(i64::from(ticks) * 1_000_000_000 / freq)
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

/// Numeric order of the module
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct ModuleId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct InputId(pub u16);

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Archive, Serialize, Deserialize)]
pub enum EventData {
    // Variants used when reading from raw event sources.

    /// Neutron with no additional time or position information.
    RawNeutron = 0x0,
    /// Signal edge without additional information.
    RawEdge { up: bool } = 0x10,
    RawAnalog1 { value1: u32, value2: f64 } = 0x20,
    RawAnalog2 { value1: u32, value2: f32, value3: f32 } = 0x21,
    RawDigital { value1: u32, value2: u32, value3: u32 } = 0x22,
    RawData { value: [u8; 14], len: u8 } = 0x30,
    Heartbeat = 0x40,

    // Variants used after processing, with more detailed information.

    /// Neutron with associated position and time bin information.
    Neutron { x: u32, y: u32, t: u32 } = 0x80,
    /// Monitor count.
    Monitor { index: u32 } = 0x90,
    /// T-zero signal (usually chopper).
    Tzero = 0x91,
    /// Gate signal.
    Gate { up: bool } = 0x92,
    /// Auxiliary signal.
    AuxSignal { value: u32, up: bool } = 0x93,
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

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct Event {
    // Do not change the structure, the serialization format depends on it.
    pub time: EventTime,
    pub rel_time: EventTime,  // zeroed until determined
    pub flags: EventFlags,
    pub module: ModuleId,
    pub input: InputId,
    pub data: EventData,
}

impl Event {
    pub fn new(time: EventTime, rel_time: EventTime, module: ModuleId, input: InputId,
               flags: EventFlags, data: EventData) -> Self {
        Self { time, rel_time, module, input, flags, data }
    }
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Event(time={:.9}, flags={:#x}, module={}, input={}, data={:?})",
               self.time.0 as f64 / 1_000_000_000.0, self.flags.0,
               self.module.0, self.input.0, self.data)
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.time.0.cmp(&other.time.0))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time.0.cmp(&other.time.0)
    }
}
