// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::time::Instant;
use anyhow::Context;
use itertools::Itertools;
use crate::lprintln;
use crate::command::ModuleId;
use crate::error::UResult;
use crate::event::{Event, EventData, EventTime};
use crate::params::HasParams;
use super::{Output, OutputCommon};

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
                lprintln!(INFO, [self.name] "{}", ev.dump());
            }
        }
        Ok(())
    }
}
