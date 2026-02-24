// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::VecDeque;
use std::io::Read;
use std::net::TcpStream;
use anyhow::anyhow;
use byteorder::{ByteOrder, LE};
use crate::error::{UError, UResult};
use crate::event::{ModuleId, InputId, Event, EventTime, EventFlags, EventData};
use crate::util::resolve;

const PACKET_NORMAL:     u32 = 0x1000;
const PACKET_NORM_FAKE:  u32 = 0x1100;
const PACKET_DIAG:       u32 = 0x3000;
const PACKET_DIAG_FAKE:  u32 = 0x3100;
const PACKET_HEARTBT:    u32 = 0x5000;
const MAX_PACKET_SIZE: usize = 65536;

pub struct GeInput {
    module: ModuleId,
    socket: TcpStream,
    buffer: VecDeque<Event>,
    is_ts: bool,
}

fn read_time(buf: &[u8]) -> EventTime {
    let sec = LE::read_u32(&buf[0..4]);
    let nsec = LE::read_u32(&buf[4..8]);
    EventTime::from_sec_nsec(sec, nsec)
}

impl crate::input::Input for GeInput {
    fn description(&self) -> String {
        format!(
            "GE {} module {} at {}",
            if self.is_ts { "TS" } else { "EP" },
            self.module.0,
            self.socket.peer_addr().map(|x| x.to_string()).unwrap_or("?".into()),
        )
    }

    fn read_event(&mut self) -> UResult<Event> {
        if let Some(ev) = self.buffer.pop_front() {
            return Ok(ev);
        }

        // read header
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        // TODO: this blocks if no data is available. OK?
        // TODO: handle reconnect if necessary
        self.socket.read_exact(&mut buffer[..16])?;
        let len = LE::read_u32(&buffer) as usize;
        let pktype = LE::read_u32(&buffer[4..]);

        if len == 0 {
            if pktype == PACKET_HEARTBT {
                return Ok(Event::new(
                    read_time(&buffer[8..]),
                    self.module,
                    InputId(0),
                    EventFlags::None,
                    EventData::Heartbeat,
                ));
            }
            return Err(anyhow!("Received empty packet of type {:#x}", pktype).into());
        }

        // read the rest
        self.socket.read_exact(&mut buffer[..len])?;
        let (evlen, flags) = match pktype {
            PACKET_NORMAL => (12, EventFlags::None),
            PACKET_NORM_FAKE => (12, EventFlags::Fake),
            PACKET_DIAG => (24, EventFlags::None),
            PACKET_DIAG_FAKE  => (24, EventFlags::Fake),
            _ => return Err(anyhow!("Unsupported packet type {:#x}", pktype).into()),
        };
        let mut offset = 24;
        let nevents = (len - offset) / evlen;
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
            self.buffer.push_back(Event::new(
                read_time(&buffer[offset..]),
                self.module,
                InputId(detid as u16),
                flags,
                data,
            ));
            offset += evlen;
        }
        Ok(self.buffer.pop_front().expect("no events in nonempty packet?"))
    }
}

impl GeInput {
    pub fn new(module: ModuleId, addr: String, ts: bool) -> UResult<Self> {
        let socket = TcpStream::connect(resolve(&addr)?)
            .map_err(UError::SourceInit)?;
        Ok(Self { module, socket, is_ts: ts,
                  buffer: VecDeque::with_capacity(32) })
    }
}
