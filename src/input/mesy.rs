// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use anyhow::Context;
use byteorder::{ByteOrder, BE};
use crate::lprintln;
use crate::command::{Command, CommandReply};
use crate::config::{MesyConfig, SourceConfig};
use crate::error::UResult;
use crate::event::{ModuleId, Event, EventFlags, EventData, EventTime, InputId};
use crate::input::{ReplayFile, DumpHandler};
use super::{Source, Input, InputCommon, UdpReader};

const MAX_PACKET_SIZE: usize = 2048;
const BEG_MARKER: &[u8] = b"\x00\x00\x55\x55\xaa\xaa\xff\xff";
const PKT_MARKER: &[u8] = b"\x00\x00\xff\xff\x55\x55\xaa\xaa";
const END_MARKER: &[u8] = b"\xff\xff\xaa\xaa\x55\x55\x00\x00";
const FILE_START: &[u8] = b"mesytec ";

pub struct MesyInput<S> {
    source: S,
    module: ModuleId,
    dump: DumpHandler,
}

impl MesyInput<()> {
    pub fn start(config: MesyConfig, common: InputCommon) -> UResult<()> {
        match &config.source {
            SourceConfig::IP(addr) => MesyInput::start_with_source(
                UdpReader::from_config(addr)?, config, common),
            SourceConfig::File(path) => MesyInput::start_with_source(
                ReplayFile::from_config(path)?, config, common),
        }
    }
}

impl<S: MesySource> MesyInput<S> {
    fn start_with_source(source: S, _config: MesyConfig, common: InputCommon) -> UResult<()> {
        let input = Self {
            source,
            module: common.module,
            dump: Default::default(),
        };
        input.start_main_loop(common)?;
        Ok(())
    }
}

impl<S: MesySource> Input for MesyInput<S> {
    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        match cmd {
            Command::SetRawDump { enable, path } => self.dump.configure(enable, path)?,
            _ => ()
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        self.dump.start(self.module, &run_id)?;
        self.dump.write(b"mesytec psd listmode data\nheader length: 2 lines \n")?;
        self.dump.write(BEG_MARKER)?;
        self.source.reset()?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.write(END_MARKER)?;
        self.dump.stop();
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Option<Vec<Event>>> {
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        let n = match self.source.get_packet(&mut buffer) {
            Ok(0) => return Ok(Some(vec![])),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(Some(vec![])),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => Err(e).context("Reading packet from source")?,
        };
        self.dump.write(&buffer[..n])?;
        self.dump.write(PKT_MARKER)?;

        let btype = BE::read_u16(&buffer[2..4]);
        if btype >> 15 != 0 {
            // not a data buffer
            lprintln!(WARN, "Mesy: got a nondata buffer?");
            return Ok(Some(vec![]));
        }
        let nwords = BE::read_u16(&buffer[0..2]) as usize;
        let nevents = (nwords*2 - 42) / 6;
        let mcpd_id = buffer[10] as u64;
        let pkt_ts = read_48bit(&buffer[12..18]);
        let status = buffer[11];
        if status & 1 != 1 {
            lprintln!(WARN, "Mesy: got event buffer but daq stopped?");
        }

        let mut events = Vec::with_capacity(nevents);
        for i in 0..nevents {
            let data = read_48bit(&buffer[42 + 6*i..]);
            let ts = pkt_ts + (data & 0x7ffff);    // 19bit
            let event = if data >> 47 == 1 {
                // trigger event
                let data_id = (data >> 40) & 0b1111;
                Event::new(
                    EventTime::from_clock(ts as i64, 10_000_000), // 10MHz clock
                    EventTime::zero(),
                    self.module,
                    InputId(data_id as u32),
                    EventFlags::None,
                    EventData::RawEdge { up: true }
                )
            } else {
                // neutron event
                let mod_id = (data >> 44) & 0b111;
                let slot_id = (data >> 39) & 0b11111;

                let ampl = (data >> 29) & 0x3ff;
                let ypos = (data >> 19) & 0x3ff;
                // This is for MPSD (8 tubes/MPSD, 8 MPSD/MCPD).
                let xpos = mcpd_id << 6 | mod_id << 3 | slot_id;

                Event::new(
                    EventTime::from_clock(ts as i64, 10_000_000), // 10MHz clock
                    EventTime::zero(),
                    self.module,
                    InputId(xpos as u32),
                    EventFlags::None,
                    EventData::RawDigital { value1: ypos as u32, value2: ampl as u32, value3: 0 }
                )
            };
            events.push(event);
        }

        Ok(Some(events))
    }
}

fn read_48bit(buf: &[u8]) -> u64 {
    let s3 = BE::read_u16(&buf[4..6]) as u64;
    let s2 = BE::read_u16(&buf[2..4]) as u64;
    let s1 = BE::read_u16(&buf[0..2]) as u64;
    s3 << 32 | s2 << 16 | s1
}

pub trait MesySource: Source {
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

impl MesySource for UdpReader {
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buffer)
    }
}

impl MesySource for ReplayFile {
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let head = &mut buffer[..8];
        self.read_exact(head)?;
        if head == FILE_START {
            // skip header, TODO hardcoded for two lines
            let mut linebreaks = 0;
            loop {
                let mut byte = [0_u8; 1];
                self.read_exact(&mut byte)?;
                if byte[0] == b'\n' {
                    linebreaks += 1;
                    if linebreaks == 2 {
                        break;
                    }
                }
            }
            self.read_exact(head)?;
            if head != BEG_MARKER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid file header"));
            }
            Ok(0)
        } else if head == END_MARKER {
            Ok(0)
        } else {
            let nwords = BE::read_u16(&buffer[..2]) as usize;
            if 2*nwords > buffer.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                                          "Packet too large for buffer or invalid packet"));
            }
            self.read_exact(&mut buffer[8..][..nwords*2 - 8])?;
            let mut foot = [0; 8];
            self.read_exact(&mut foot)?; // read the packet end marker
            if foot != PKT_MARKER {
                dbg!(foot);
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid packet end marker"));
            }
            Ok(nwords*2)
        }
    }
}
