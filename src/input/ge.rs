// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::net::TcpStream;
use anyhow::{anyhow, Context};
use byteorder::{ByteOrder, LE};
use crate::command::{Command, CommandReply};
use crate::config::{GEConfig, SourceConfig};
use crate::error::{UError, UResult};
use crate::input::{ReplayFile, DumpHandler};
use crate::event::{ModuleId, InputId, Event, EventTime, EventFlags, EventData};
use super::{Source, Input, InputCommon};

const PACKET_NORMAL:     u32 = 0x1000;
const PACKET_NORM_FAKE:  u32 = 0x1100;
const PACKET_DIAG:       u32 = 0x3000;
const PACKET_DIAG_FAKE:  u32 = 0x3100;
const PACKET_HEARTBT:    u32 = 0x5000;
const MAX_PACKET_SIZE: usize = 65536;

pub struct GeInput<S> {
    source: S,
    module: ModuleId,
    dump: DumpHandler,
    is_ts: bool,
    // last_event_at: EventTime,
}

fn read_time(buf: &[u8]) -> EventTime {
    let sec = LE::read_u32(&buf[0..4]);
    let nsec = LE::read_u32(&buf[4..8]);
    EventTime::from_sec_nsec(sec, nsec)
}

impl GeInput<()> {
    pub fn start(config: GEConfig, common: InputCommon) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => GeInput::start_with_source(
                TcpStream::from_config(addr)?, config, common),
            SourceConfig::File(path) => GeInput::start_with_source(
                ReplayFile::from_config(path)?, config, common),
        }
    }
}

impl<S: Source> GeInput<S> {
    fn start_with_source(source: S, config: GEConfig, common: InputCommon) -> UResult<()> {
        let input = Self {
            source,
            dump: Default::default(),
            module: common.module,
            is_ts: config.timestamper,
            // last_event_at: EventTime::zero(),
        };
        input.start_main_loop(common)?;
        Ok(())
    }
}

impl<S: Source> Input for GeInput<S> {
    fn description(&self) -> String {
        format!(
            "GE {} module {} at {}",
            if self.is_ts { "TS" } else { "EP" },
            self.module.0,
            self.source.description()
        )
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        if let Command::SetRawDump { enable, path } = cmd {
            self.dump.configure(enable, path)?;
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        self.dump.start(self.module, &run_id)?;
        self.source.reset()?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.stop();
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        // read header
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        match self.source.read_exact(&mut buffer[..16]) {
            Ok(_) => {},
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(vec![]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(UError::InputEnded),
            // TODO: handle reconnect if necessary
            Err(e) => Err(e).context("Reading from source")?,
        }
        let len = LE::read_u32(&buffer) as usize;
        let pktype = LE::read_u32(&buffer[4..]);

        if len == 0 {
            if pktype == PACKET_HEARTBT {
                self.dump.write(&buffer[..16])?;
                return Ok(vec![Event::new(
                    read_time(&buffer[8..]),
                    EventTime::zero(),
                    self.module,
                    InputId(0),
                    EventFlags::None,
                    EventData::Heartbeat,
                )]);
            }
            return Err(anyhow!("Received empty packet of type {:#x}", pktype).into());
        }

        // read the rest
        self.source.read_exact(&mut buffer[16..][..len])
                   .context("Reading packet content")?;
        let (evlen, flags) = match pktype {
            PACKET_NORMAL => (12, EventFlags::None),
            PACKET_NORM_FAKE => (12, EventFlags::Fake),
            PACKET_DIAG => (24, EventFlags::None),
            PACKET_DIAG_FAKE  => (24, EventFlags::Fake),
            _ => return Err(anyhow!("Unsupported packet type {:#x}", pktype))?,
        };
        let nevents = (len - 24) / evlen;
        let mut offset = 40;
        let mut events = Vec::with_capacity(nevents);
        for _ in 0..nevents {
            let detid = LE::read_u32(&buffer[offset+8..]);
            let data = if self.is_ts {
                EventData::RawEdge { up: detid & 0x8000_0000 != 0 }
            } else if pktype == PACKET_DIAG || pktype == PACKET_DIAG_FAKE {
                let max_heights = LE::read_u32(&buffer[offset+12..]);
                let a_integrated = LE::read_u32(&buffer[offset+16..]);
                let b_integrated = LE::read_u32(&buffer[offset+20..]);
                EventData::RawDigital { value1: max_heights,
                                        value2: a_integrated,
                                        value3: b_integrated }
            } else {
                EventData::RawNeutron
            };
            let event = Event::new(
                read_time(&buffer[offset..]),
                EventTime::zero(),
                self.module,
                InputId(detid),
                flags,
                data,
            );
            events.push(event);
            offset += evlen;
        }
        events.sort();

        self.dump.write(&buffer[..16+len])?;

        // if events[0].time < self.last_event_at {
        //     crate::lprintln!(WARN, "Received out-of-order events with time {} (current ts {}), jump {}",
        //                      events[0].time, self.last_event_at, self.last_event_at - events[0].time);
        // }
        // self.last_event_at = events.last().unwrap().time;

        Ok(events)
    }
}
