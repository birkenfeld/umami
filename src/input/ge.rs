// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fs::File;
use std::net::TcpStream;
use anyhow::anyhow;
use byteorder::{ByteOrder, LE};
use crate::config::{GEConfig, SourceConfig};
use crate::error::{UError, UResult};
use crate::event::{ModuleId, InputId, Event, EventTime, EventFlags, EventData};
use super::{Source, Input, InputChannels};

const PACKET_NORMAL:     u32 = 0x1000;
const PACKET_NORM_FAKE:  u32 = 0x1100;
const PACKET_DIAG:       u32 = 0x3000;
const PACKET_DIAG_FAKE:  u32 = 0x3100;
const PACKET_HEARTBT:    u32 = 0x5000;
const MAX_PACKET_SIZE: usize = 65536;

pub struct GeInput<S> {
    source: S,
    module: ModuleId,
    is_ts: bool,
    channels: InputChannels,
    last_event_at: EventTime,
}

fn read_time(buf: &[u8]) -> EventTime {
    let sec = LE::read_u32(&buf[0..4]);
    let nsec = LE::read_u32(&buf[4..8]);
    EventTime::from_sec_nsec(sec, nsec)
}

impl GeInput<()> {
    pub fn init(module: ModuleId, config: GEConfig, channels: InputChannels) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => GeInput::init_with_source(TcpStream::from_config(addr)?,
                                                             module, config, channels),
            SourceConfig::File(path) => GeInput::init_with_source(File::from_config(path)?,
                                                               module, config, channels),
        }
    }
}

impl<S: Source> GeInput<S> {
    fn init_with_source(source: S, module: ModuleId, config: GEConfig,
                        channels: InputChannels) -> UResult<()> {
        let input = Self { source, module, is_ts: config.timestamper, channels,
                           last_event_at: EventTime::zero() };
        input.start_event_thread();
        Ok(())
    }
}

impl<S: Source> Input for GeInput<S> {
    fn channels(&self) -> &InputChannels {
        &self.channels
    }

    fn description(&self) -> String {
        format!(
            "GE {} module {} at {}",
            if self.is_ts { "TS" } else { "EP" },
            self.module.0,
            self.source.description()
        )
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        // read header
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        // TODO: this blocks if no data is available. OK?
        // TODO: handle reconnect if necessary
        match self.source.read_exact(&mut buffer[..16]) {
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(UError::ReadInput(e).into()),
        }
        let len = LE::read_u32(&buffer) as usize;
        let pktype = LE::read_u32(&buffer[4..]);

        if len == 0 {
            if pktype == PACKET_HEARTBT {
                return Ok(Some(vec![Event::new(
                    read_time(&buffer[8..]),
                    self.module,
                    InputId(0),
                    EventFlags::None,
                    EventData::Heartbeat,
                )]));
            }
            return Err(anyhow!("Received empty packet of type {:#x}", pktype).into());
        }

        // read the rest
        self.source.read_exact(&mut buffer[..len])?;
        let (evlen, flags) = match pktype {
            PACKET_NORMAL => (12, EventFlags::None),
            PACKET_NORM_FAKE => (12, EventFlags::Fake),
            PACKET_DIAG => (24, EventFlags::None),
            PACKET_DIAG_FAKE  => (24, EventFlags::Fake),
            _ => return Err(anyhow!("Unsupported packet type {:#x}", pktype).into()),
        };
        let mut offset = 24;
        let nevents = (len - offset) / evlen;
        let mut events = Vec::with_capacity(nevents);
        for _ in 0..nevents {
            let detid = LE::read_u32(&buffer[offset+8..]);
            let data = if self.is_ts {
                EventData::AuxSignal { up: detid & 0x8000_0000 != 0 }
            } else if pktype == PACKET_DIAG || pktype == PACKET_DIAG_FAKE {
                let max_heights = LE::read_u32(&buffer[offset+12..]);
                let a_integrated = LE::read_u32(&buffer[offset+16..]);
                let b_integrated = LE::read_u32(&buffer[offset+20..]);
                EventData::Digital { value1: max_heights,
                                     value2: a_integrated,
                                     value3: b_integrated }
            } else {
                EventData::Neutron
            };
            events.push(Event::new(
                read_time(&buffer[offset..]),
                self.module,
                InputId(detid as u16),
                flags,
                data,
            ));
            offset += evlen;
        }
        events.sort();
        if events[0].time < self.last_event_at {
            crate::lprintln!(WARN, "Received out-of-order events with time {} (current ts {}), jump {}",
                             events[0].time, self.last_event_at, self.last_event_at - events[0].time);
        }
        self.last_event_at = events.last().unwrap().time;
        Ok(Some(events))
    }
}
