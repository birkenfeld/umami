// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fmt::{Debug, Formatter, Result as FmtResult};
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Debug, Clone, Copy)]
#[derive(Archive, Serialize, Deserialize)]
pub struct EventTime(
    // Absolute time in nanoseconds (TODO)
    u64
);

impl EventTime {
    pub fn now() -> Self { todo!() }
    pub fn from_sec_nsec(sec: u32, nsec: u32) -> Self {
        Self(sec as u64 * 1_000_000_000 + nsec as u64)
    }
}

#[derive(Debug, Clone, Copy)]
#[derive(Archive, Serialize, Deserialize)]
pub struct ModuleId(pub u16);

#[derive(Debug, Clone, Copy)]
#[derive(Archive, Serialize, Deserialize)]
pub struct InputId(pub u16);

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
#[derive(Archive, Serialize, Deserialize)]
pub enum EventData {
    Neutron = 0x0,
    Monitor = 0x20,
    AuxSignal { up: bool } = 0x30,
    Analog1 { value1: u32, value2: f64 } = 0x40,
    Analog2 { value1: u32, value2: f32, value3: f32 } = 0x41,
    Digital { value1: u32, value2: u32, value3: u32 } = 0x42,
    AuxData { value: [u8; 14], len: u8 } = 0x50,
    Heartbeat = 0xFF,
}

#[bitflag_attr::bitflag(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub enum EventFlags {
    None = 0,
    Fake = 0x1000,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[derive(Archive, Serialize, Deserialize)]
pub struct Event {
    pub time: EventTime,
    pub flags: EventFlags,
    pub module: ModuleId,
    pub input: InputId,
    pub data: EventData,
}

impl Event {
    pub fn new(time: EventTime, module: ModuleId, input: InputId,
               flags: EventFlags, data: EventData) -> Self {
        Self { time, module, input, flags, data }
    }
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Event(time={:.9}, flags={:#x}, module={}, input={}, data={:?})",
               self.time.0 as f64 / 1_000_000_000.0, self.flags.0,
               self.module.0, self.input.0, self.data)
    }
}
