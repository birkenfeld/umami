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
pub struct EventTime(pub(crate) i64);

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

/// Amplitude of the event, e.g. pulse height or time-over-threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
pub struct Amplitude(pub u32);

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Archive, Serialize, Deserialize)]
// Note: if you change this, update the src/expr.rs language accordingly.
pub enum EventType {
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

impl Eq for EventType {}

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
pub struct EventHisto {
    pub x: u16,
    pub y: u16,
    pub t: u16,
    pub i: u16,
}

impl EventHisto {
    pub fn zero() -> Self {
        Self { x: 0, y: 0, t: 0, i: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[derive(Archive, Serialize, Deserialize)]
// Note: if you change this, update the src/expr.rs language accordingly.
pub struct Event {
    // Do not change the structure, the serialization format depends on it.
    pub time: EventTime,
    pub rel_time: EventTime,  // zeroed until determined
    pub raw: (u32, u32),      // raw data from hardware, e.g. for debugging
    pub channel: ChannelId,
    pub ampl: Amplitude,
    // histogram coordinates
    pub histo: EventHisto,
    pub flags: EventFlags,
    pub evtype: EventType,
}

impl Event {
    pub fn new(evtype: EventType) -> Self {
        Self { evtype,
               channel: ChannelId(0),
               time: EventTime::zero(),
               rel_time: EventTime::zero(),
               flags: EventFlags::None,
               histo: EventHisto::zero(),
               ampl: Amplitude(0),
               raw: (0, 0) }
    }

    pub fn with_channel(mut self, channel: u32) -> Self {
        self.channel = ChannelId(channel);
        self
    }

    pub fn with_abs_time_and_offset(mut self, time: EventTime, off: EventTime) -> Self {
        self.time = time;
        self.rel_time = time + off;
        self.flags.set(EventFlags::HasRelTime);
        self
    }

    pub fn with_abs_time(mut self, time: EventTime) -> Self {
        self.time = time;
        self
    }

    pub fn with_rel_time(mut self, rel_time: EventTime) -> Self {
        self.rel_time = rel_time;
        self.flags.set(EventFlags::HasRelTime);
        self
    }

    pub fn with_ampl(mut self, ampl: u32) -> Self {
        self.ampl = Amplitude(ampl);
        self
    }

    pub fn with_raw(mut self, a: u32, b: u32) -> Self {
        self.raw = (a, b);
        self
    }

    pub fn with_flags(mut self, flags: EventFlags) -> Self {
        self.flags = flags;
        self
    }

    pub fn dump(&self) -> DumpEvent<'_> {
        DumpEvent(self)
    }
}

impl Debug for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Event(time={:.9}, rel_time={:.9}, flags={:#x}, channel={}, evtype={:?})",
               self.time.0 as f64 / 1_000_000_000.0,
               self.rel_time.0 as f64 / 1_000_000_000.0,
               self.flags.0, self.channel.0, self.evtype)
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
        match ev.evtype {
            EventType::Neutron =>
                write!(f, "Neutron"),
            EventType::Edge { up } =>
                write!(f, "Edge      {}", if up { "up" } else { "down" }),
            EventType::Heartbeat =>
                write!(f, "Heartbeat"),
            EventType::Monitor =>
                write!(f, "Monitor"),
            EventType::Tzero =>
                write!(f, "T-zero"),
            EventType::Gate { up } =>
                write!(f, "Gate      {}", if up { "up" } else { "down" }),
            EventType::AuxSignal { num } =>
                write!(f, "AuxSignal {num}"),
            EventType::Void =>
                write!(f, "Void"),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;
    pub fn neutron(time_ns: i64, channel: u32) -> Event {
        Event::new(EventType::Neutron).with_channel(channel).with_abs_time(EventTime(time_ns))
    }

    pub fn edge(time_ns: i64, channel: u32, up: bool) -> Event {
        Event::new(EventType::Edge { up }).with_channel(channel).with_abs_time(EventTime(time_ns))
    }

    pub fn tzero(time_ns: i64) -> Event {
        Event::new(EventType::Tzero).with_abs_time(EventTime(time_ns))
    }

    pub fn gate(time_ns: i64, up: bool) -> Event {
        Event::new(EventType::Gate { up }).with_abs_time(EventTime(time_ns))
    }

    pub fn aux(time_ns: i64, num: u8) -> Event {
        Event::new(EventType::AuxSignal { num }).with_abs_time(EventTime(time_ns))
    }

    pub fn heartbeat(time_ns: i64) -> Event {
        Event::new(EventType::Heartbeat).with_abs_time(EventTime(time_ns))
    }

    pub fn void(time_ns: i64) -> Event {
        Event::new(EventType::Void).with_abs_time(EventTime(time_ns))
    }

    pub fn neutron_xy(time_ns: i64, channel: u32, x: u16, y: u16) -> Event {
        let mut ev = Event::new(EventType::Neutron)
            .with_channel(channel).with_abs_time(EventTime(time_ns));
        ev.histo.x = x;
        ev.histo.y = y;
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn test_event_size() {
        assert_eq!(size_of::<Event>(), 48);
    }

    #[test]
    fn test_event_time_api() {
        // constructors
        assert_eq!(EventTime::zero(), EventTime(0));
        assert_eq!(EventTime::from_sec_nsec(1, 500_000_000), EventTime(1_500_000_000));
        assert_eq!(EventTime::from_sec_nsec(0, 0), EventTime::zero());
        assert_eq!(EventTime::from_floating_sec(1.5), EventTime(1_500_000_000));
        assert_eq!(EventTime::from_ticks(100, 10i64), EventTime(1000));
        assert_eq!(EventTime::from_ticks(100, 0i64), EventTime::zero());
        assert_eq!(EventTime::from_clock(1_000_000, 500_000i64), EventTime(500_000_000));
        assert_eq!(EventTime::from_clock(1_000_000, 0i64), EventTime::zero());
        assert_eq!(EventTime::MAX, EventTime(i64::MAX));

        // display
        assert_eq!(format!("{}", EventTime(1_500_000_000)), "1.500000000s");
        assert_eq!(format!("{}", EventTime(0)), "0.000000000s");

        // arithmetic
        let a = EventTime(1_000_000_000);
        let b = EventTime(500_000_000);
        assert_eq!(a + b, EventTime(1_500_000_000));
        assert_eq!(a - b, EventTime(500_000_000));
        assert_eq!(b - a, EventTime(-500_000_000));

        // ordering
        assert!(EventTime(100) < EventTime(200));
        assert!(EventTime(200) > EventTime(100));
        assert_eq!(EventTime(100), EventTime(100));

        // conversion to f64 seconds
        let t: f64 = EventTime(1_500_000_000).into();
        assert!((t - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_event_ordering() {
        // Ord/PartialOrd only compare `time`, ignoring channel
        let e1 = test_utils::neutron(100, 0);
        let e2 = test_utils::neutron(200, 0);
        let e3 = test_utils::neutron(100, 1);
        assert!(e1 < e2);
        assert!(e2 > e1);
        assert_eq!(e1.cmp(&e3), std::cmp::Ordering::Equal);

        let mut events = [
            test_utils::neutron(300, 0),
            test_utils::neutron(100, 0),
            test_utils::neutron(200, 0),
        ];
        events.sort();
        assert_eq!(events[0].time, EventTime(100));
        assert_eq!(events[1].time, EventTime(200));
        assert_eq!(events[2].time, EventTime(300));
    }

    #[test]
    fn test_event_data_equality() {
        assert_eq!(EventType::Neutron, EventType::Neutron);
        assert_ne!(EventType::Neutron, EventType::Edge { up: true });
        assert_eq!(EventType::Edge { up: true }, EventType::Edge { up: true });
        assert_ne!(EventType::Edge { up: true }, EventType::Edge { up: false });
    }

    #[test]
    fn test_event_flags() {
        let empty = EventFlags::empty();
        assert!(empty.is_empty());
        assert!(!empty.contains(EventFlags::HasRelTime));
        assert_eq!(format!("{empty}"), "-");

        let rt = EventFlags::HasRelTime;
        assert!(rt.contains(EventFlags::HasRelTime));
        assert!(!rt.contains(EventFlags::Fake));
        assert_eq!(format!("{rt}"), "RT");
        assert_eq!(format!("{}", EventFlags::Fake), "F");

        let combined = rt | EventFlags::Fake;
        assert!(combined.contains(EventFlags::HasRelTime));
        assert!(combined.contains(EventFlags::Fake));
        assert_eq!(format!("{combined}"), "RT|F");
    }

    #[test]
    fn test_event_new() {
        let ev = Event::new(EventType::Neutron)
            .with_channel(42)
            .with_abs_time(EventTime(100))
            .with_rel_time(EventTime(50))
            .with_ampl(1234);
        assert_eq!(ev.time, EventTime(100));
        assert_eq!(ev.rel_time, EventTime(50));
        assert_eq!(ev.channel, ChannelId(42));
        assert_eq!(ev.flags, EventFlags::HasRelTime);
        assert_eq!(ev.evtype, EventType::Neutron);
        assert_eq!(ev.ampl, Amplitude(1234));
        assert_eq!(ev.histo.x, 0);
        assert_eq!(ev.histo.y, 0);
        assert_eq!(ev.histo.t, 0);
        assert_eq!(ev.histo.i, 0);
    }

    #[test]
    fn test_event_helpers() {
        let ev = test_utils::neutron(100, 5);
        assert_eq!(ev.evtype, EventType::Neutron);
        assert_eq!(ev.time, EventTime(100));
        assert_eq!(ev.channel, ChannelId(5));

        let ev = test_utils::edge(200, 3, true);
        assert_eq!(ev.evtype, EventType::Edge { up: true });
        assert_eq!(ev.channel, ChannelId(3));

        let ev = test_utils::tzero(300);
        assert_eq!(ev.evtype, EventType::Tzero);

        let ev = test_utils::gate(400, false);
        assert_eq!(ev.evtype, EventType::Gate { up: false });

        let ev = test_utils::aux(500, 2);
        assert_eq!(ev.evtype, EventType::AuxSignal { num: 2 });

        let ev = test_utils::heartbeat(600);
        assert_eq!(ev.evtype, EventType::Heartbeat);

        let ev = test_utils::void(700);
        assert_eq!(ev.evtype, EventType::Void);

        let ev = test_utils::neutron_xy(800, 1, 10, 20);
        assert_eq!(ev.histo.x, 10);
        assert_eq!(ev.histo.y, 20);
    }
}
