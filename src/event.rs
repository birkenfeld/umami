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

#[cfg(test)]
pub(crate) mod test_utils {
    use super::*;

    pub fn neutron(time_ns: i64, channel: u32) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(channel),
                    EventFlags::empty(), EventData::Neutron, 0)
    }

    pub fn edge(time_ns: i64, channel: u32, up: bool) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(channel),
                    EventFlags::empty(), EventData::Edge { up }, 0)
    }

    pub fn tzero(time_ns: i64) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(0),
                    EventFlags::empty(), EventData::Tzero, 0)
    }

    pub fn gate(time_ns: i64, up: bool) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(0),
                    EventFlags::empty(), EventData::Gate { up }, 0)
    }

    pub fn aux(time_ns: i64, num: u8) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(0),
                    EventFlags::empty(), EventData::AuxSignal { num }, 0)
    }

    pub fn heartbeat(time_ns: i64) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(0),
                    EventFlags::empty(), EventData::Heartbeat, 0)
    }

    pub fn void(time_ns: i64) -> Event {
        Event::new(EventTime(time_ns), EventTime::zero(), ChannelId(0),
                    EventFlags::empty(), EventData::Void, 0)
    }

    pub fn neutron_xy(time_ns: i64, channel: u32, x: u32, y: u32) -> Event {
        let mut ev = neutron(time_ns, channel);
        ev.x = x;
        ev.y = y;
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
    fn test_event_time_constructors() {
        assert_eq!(EventTime::zero(), EventTime(0));
        assert_eq!(EventTime::from_sec_nsec(1, 500_000_000), EventTime(1_500_000_000));
        assert_eq!(EventTime::from_sec_nsec(0, 0), EventTime::zero());
        assert_eq!(EventTime::from_floating_sec(1.5), EventTime(1_500_000_000));
        assert_eq!(EventTime::from_ticks(100, 10i64), EventTime(1000));
        assert_eq!(EventTime::from_ticks(100, 0i64), EventTime::zero());
        assert_eq!(EventTime::from_clock(1_000_000, 500_000i64), EventTime(500_000_000));
        assert_eq!(EventTime::from_clock(1_000_000, 0i64), EventTime::zero());
        assert_eq!(EventTime::MAX, EventTime(i64::MAX));
    }

    #[test]
    fn test_event_time_display() {
        assert_eq!(format!("{}", EventTime(1_500_000_000)), "1.500000000s");
        assert_eq!(format!("{}", EventTime(0)), "0.000000000s");
    }

    #[test]
    fn test_event_time_arithmetic() {
        let a = EventTime(1_000_000_000);
        let b = EventTime(500_000_000);
        assert_eq!(a + b, EventTime(1_500_000_000));
        assert_eq!(a - b, EventTime(500_000_000));
        assert_eq!(b - a, EventTime(-500_000_000));
    }

    #[test]
    fn test_event_time_ordering() {
        assert!(EventTime(100) < EventTime(200));
        assert!(EventTime(200) > EventTime(100));
        assert_eq!(EventTime(100), EventTime(100));
    }

    #[test]
    fn test_event_time_to_f64() {
        let t: f64 = EventTime(1_500_000_000).into();
        assert!((t - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_event_ordering() {
        let e1 = test_utils::neutron(100, 0);
        let e2 = test_utils::neutron(200, 0);
        let e3 = test_utils::neutron(100, 1);
        assert!(e1 < e2);
        assert!(e2 > e1);
        assert_eq!(e1.cmp(&e3), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_event_sort() {
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
        assert_eq!(EventData::Neutron, EventData::Neutron);
        assert_ne!(EventData::Neutron, EventData::Edge { up: true });
        assert_eq!(EventData::Edge { up: true }, EventData::Edge { up: true });
        assert_ne!(EventData::Edge { up: true }, EventData::Edge { up: false });
    }

    #[test]
    fn test_event_flags_bitwise() {
        let empty = EventFlags::empty();
        assert!(empty.is_empty());
        assert!(!empty.contains(EventFlags::HasRelTime));

        let rt = EventFlags::HasRelTime;
        assert!(rt.contains(EventFlags::HasRelTime));
        assert!(!rt.contains(EventFlags::Fake));

        let combined = rt | EventFlags::Fake;
        assert!(combined.contains(EventFlags::HasRelTime));
        assert!(combined.contains(EventFlags::Fake));
    }

    #[test]
    fn test_event_flags_display() {
        assert_eq!(format!("{}", EventFlags::empty()), "-");
        assert_eq!(format!("{}", EventFlags::HasRelTime), "RT");
        assert_eq!(format!("{}", EventFlags::Fake), "F");
        assert_eq!(format!("{}", EventFlags::HasRelTime | EventFlags::Fake), "RT|F");
    }

    #[test]
    fn test_event_new() {
        let ev = Event::new(
            EventTime(100), EventTime(50), ChannelId(42),
            EventFlags::HasRelTime, EventData::Neutron, 1234,
        );
        assert_eq!(ev.time, EventTime(100));
        assert_eq!(ev.rel_time, EventTime(50));
        assert_eq!(ev.channel, ChannelId(42));
        assert_eq!(ev.flags, EventFlags::HasRelTime);
        assert_eq!(ev.data, EventData::Neutron);
        assert_eq!(ev.ampl, 1234);
        assert_eq!(ev.x, 0);
        assert_eq!(ev.y, 0);
        assert_eq!(ev.t, 0);
        assert_eq!(ev.i, 0);
    }

    #[test]
    fn test_event_helpers() {
        let ev = test_utils::neutron(100, 5);
        assert_eq!(ev.data, EventData::Neutron);
        assert_eq!(ev.time, EventTime(100));
        assert_eq!(ev.channel, ChannelId(5));

        let ev = test_utils::edge(200, 3, true);
        assert_eq!(ev.data, EventData::Edge { up: true });
        assert_eq!(ev.channel, ChannelId(3));

        let ev = test_utils::tzero(300);
        assert_eq!(ev.data, EventData::Tzero);

        let ev = test_utils::gate(400, false);
        assert_eq!(ev.data, EventData::Gate { up: false });

        let ev = test_utils::aux(500, 2);
        assert_eq!(ev.data, EventData::AuxSignal { num: 2 });

        let ev = test_utils::heartbeat(600);
        assert_eq!(ev.data, EventData::Heartbeat);

        let ev = test_utils::void(700);
        assert_eq!(ev.data, EventData::Void);

        let ev = test_utils::neutron_xy(800, 1, 10, 20);
        assert_eq!(ev.x, 10);
        assert_eq!(ev.y, 20);
    }
}
