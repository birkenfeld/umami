// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::time::Instant;
use anyhow::Context;
use itertools::Itertools;
use crate::lprintln;
use crate::command::ModuleId;
use crate::error::UResult;
use crate::event::{Event, EventType, EventTime};
use crate::params::HasParams;
use super::{Output, OutputCommon};

#[bitflag_attr::bitflag(u16)]
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventMask {
    NEUTRON = 0x1,
    MONITOR = 0x2,
    EDGE = 0x10,
    GATE = 0x20,
    TZERO = 0x40,
    AUX = 0x80,
    HEARTBEAT = 0x100,
    VOID = 0x200,

    // combined flags
    SIGNAL = 0xF,
    EDGES = 0xF0,
    OTHER = 0xF00,
    NOTNEUTRON = 0xFF0,
    ALL = 0xFFF,
}

impl EventMask {
    /// The single flag identifying which mask bit corresponds to `evtype`.
    pub fn for_event(evtype: &EventType) -> EventMask {
        match evtype {
            EventType::Neutron => EventMask::NEUTRON,
            EventType::Edge { .. } => EventMask::EDGE,
            EventType::Heartbeat => EventMask::HEARTBEAT,
            EventType::Monitor => EventMask::MONITOR,
            EventType::Tzero => EventMask::TZERO,
            EventType::Gate { .. } => EventMask::GATE,
            EventType::AuxSignal { .. } => EventMask::AUX,
            EventType::Void => EventMask::VOID,
        }
    }
}


/// Output selected events, and count out-of-order events.
#[derive(HasParams)]
pub struct DiagOutput {
    name: ModuleId,
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
    fn from_config(common: &OutputCommon, config: toml::Table) -> UResult<Self> {
        let mask = config.get("event_mask").and_then(|v| v.as_str()).unwrap_or("");
        let mask: EventMask = bitflag_attr::parser::from_text(mask)
            .with_context(|| {
                format!("Invalid event_mask: {} - valid flags are {} and can be combined with '|'",
                        mask, EventMask::all().iter_names().map(|(name, _)| name).join(", "))
            })?;
        if mask.is_empty() {
            lprintln!(INFO, [common.name] "Set an `event_mask` to print individual events");
        }

        Ok(DiagOutput {
            name: common.name,
            event_mask: mask,
            check_order: config.get("check_order")
                               .and_then(|v| v.as_bool())
                               .unwrap_or(false),
            print_every: config.get("print_every")
                               .and_then(|v| v.as_integer())
                               .unwrap_or(i64::MAX) as usize,
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
        lprintln!(INFO, [self.name] "Ran for {:.3} s, total events: {}, rate: {} ev/s",
                  time, self.ev_count, rate);
        if self.out_of_order > 0 {
            lprintln!(INFO, [self.name] "Total out of order: {}", self.out_of_order);
        }
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        self.ev_count += events.len();
        if self.ev_count >= self.debug_at {
            lprintln!(DEBUG, [self.name] "Received {} events", self.debug_at);
            self.debug_at += self.print_every;
        }

        for ev in events {
            let ev_ts = ev.time;
            if self.check_order && ev_ts < self.last_ts {
                lprintln!(INFO, [self.name]
                          "Out of order event: last_ts={:?}, ev_ts={ev_ts:?}", self.last_ts);
                self.out_of_order += 1;
            }
            self.last_ts = ev_ts;
            let display = self.event_mask.contains(EventMask::for_event(&ev.evtype));
            if display {
                lprintln!(INFO, [self.name] "{}", ev.dump());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;
    use crate::command::ModuleId;
    use crate::pipeline::PipeItem;
    use crate::channel;

    fn make_common(name: &str) -> (OutputCommon, channel::Sender<PipeItem>) {
        let (send, recv) = crate::channel::unbounded();
        let common = OutputCommon::new(ModuleId::new(name.into()), recv, None);
        (common, send)
    }

    #[test]
    fn test_diag_counting() {
        let (common, _sender) = make_common("test");
        let config = toml::Table::new();
        let mut output = DiagOutput::from_config(&common, config).unwrap();
        output.handle_start_of_run("run1").unwrap();

        let events = vec![
            test_utils::neutron(100, 0),
            test_utils::neutron(200, 0),
            test_utils::neutron(300, 0),
        ];
        output.handle_events(&events).unwrap();
        assert_eq!(output.ev_count, 3);

        output.handle_end_of_run().unwrap();
    }

    #[test]
    fn test_diag_out_of_order_detection() {
        let (common, _sender) = make_common("test");
        let mut config = toml::Table::new();
        config.insert("check_order".into(), toml::Value::Boolean(true));
        let mut output = DiagOutput::from_config(&common, config).unwrap();
        output.handle_start_of_run("run1").unwrap();

        let events = vec![
            test_utils::neutron(300, 0),
            test_utils::neutron(100, 0), // out of order
        ];
        output.handle_events(&events).unwrap();
        assert_eq!(output.out_of_order, 1);
    }

    #[test]
    fn test_diag_mask_filtering() {
        let (common, _sender) = make_common("test");
        let mut config = toml::Table::new();
        config.insert("event_mask".into(), toml::Value::String("NEUTRON".into()));
        let mut output = DiagOutput::from_config(&common, config).unwrap();
        output.handle_start_of_run("run1").unwrap();

        // mix of event types - only neutrons should match mask
        let events = vec![
            test_utils::neutron(100, 0),
            test_utils::edge(200, 0, true),
            test_utils::heartbeat(300),
        ];
        output.handle_events(&events).unwrap();
        assert_eq!(output.ev_count, 3); // all still counted regardless of mask

        // the mask itself only selects the events it was configured for
        assert!(output.event_mask.contains(EventMask::for_event(&EventType::Neutron)));
        assert!(!output.event_mask.contains(EventMask::for_event(&EventType::Edge { up: true })));
        assert!(!output.event_mask.contains(EventMask::for_event(&EventType::Heartbeat)));
    }

    #[test]
    fn test_event_mask_for_event_covers_all_types() {
        assert_eq!(EventMask::for_event(&EventType::Neutron), EventMask::NEUTRON);
        assert_eq!(EventMask::for_event(&EventType::Edge { up: true }), EventMask::EDGE);
        assert_eq!(EventMask::for_event(&EventType::Heartbeat), EventMask::HEARTBEAT);
        assert_eq!(EventMask::for_event(&EventType::Monitor), EventMask::MONITOR);
        assert_eq!(EventMask::for_event(&EventType::Tzero), EventMask::TZERO);
        assert_eq!(EventMask::for_event(&EventType::Gate { up: false }), EventMask::GATE);
        assert_eq!(EventMask::for_event(&EventType::AuxSignal { num: 1 }), EventMask::AUX);
        assert_eq!(EventMask::for_event(&EventType::Void), EventMask::VOID);

        // combined masks select exactly their constituent types
        let combo = EventMask::NEUTRON | EventMask::GATE;
        assert!(combo.contains(EventMask::for_event(&EventType::Neutron)));
        assert!(combo.contains(EventMask::for_event(&EventType::Gate { up: true })));
        assert!(!combo.contains(EventMask::for_event(&EventType::Edge { up: true })));
        assert!(EventMask::ALL.contains(EventMask::for_event(&EventType::Void)));
    }

    #[test]
    fn test_diag_print_every() {
        let (common, _sender) = make_common("test");
        let mut config = toml::Table::new();
        config.insert("print_every".into(), toml::Value::Integer(5));
        let mut output = DiagOutput::from_config(&common, config).unwrap();
        output.handle_start_of_run("run1").unwrap();

        let events: Vec<_> = (0..10).map(|i| test_utils::neutron(i * 100, 0)).collect();
        output.handle_events(&events).unwrap();
        assert_eq!(output.ev_count, 10);
        // debug_at should have been incremented once
        assert_eq!(output.debug_at, 10);
    }

    #[test]
    fn test_diag_reset_on_start() {
        let (common, _sender) = make_common("test");
        let mut config = toml::Table::new();
        config.insert("check_order".into(), toml::Value::Boolean(true));
        let mut output = DiagOutput::from_config(&common, config).unwrap();
        output.handle_start_of_run("run1").unwrap();

        let events = vec![test_utils::neutron(300, 0), test_utils::neutron(100, 0)];
        output.handle_events(&events).unwrap();
        assert_eq!(output.out_of_order, 1);

        // restart should reset
        output.handle_start_of_run("run2").unwrap();
        assert_eq!(output.out_of_order, 0);
        assert_eq!(output.ev_count, 0);
    }
}
