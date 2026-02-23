// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#[derive(Debug, Clone, Copy)]
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
pub struct ModuleId(pub u16);

#[derive(Debug, Clone, Copy)]
pub struct InputId(pub u16);

#[repr(u8)]
#[derive(Debug, Clone)]
pub enum EventData {
    Heartbeat = 0,
    Neutron = 0x10,
    Monitor = 0x20,
    Signal { up: bool } = 0x30,
    Analog1 { meta: u32, value: f64 } = 0x40,
    Analog2 { meta: u32, value1: f32, value2: f32 } = 0x41,
    AuxData { value: [u8; 14], len: u8 } = 0x50,
}

#[bitflag_attr::bitflag(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventFlags {
    None = 0,
    Fake = 0x1000,
}

#[repr(C)]
#[derive(Debug, Clone)]  // TODO: custom Debug impl
pub struct Event {
    pub time: EventTime,
    pub module: ModuleId,
    pub input: InputId,
    pub flags: EventFlags,
    pub data: EventData,
}

impl Event {
    pub fn new(time: EventTime, module: ModuleId, input: InputId,
               flags: EventFlags, data: EventData) -> Self {
        Self { time, module, input, flags, data }
    }
}
