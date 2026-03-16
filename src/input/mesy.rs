// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod cmd;

use std::io;
use anyhow::Context;
use byteorder::{ByteOrder, BE, LE};
use crate::lprintln;
use crate::command::{Command, CommandReply};
use crate::config::{MesyConfig, SourceConfig};
use crate::error::{UError, UResult};
use crate::event::{ModuleId, Event, EventFlags, EventData, EventTime, InputId};
use crate::input::{ReplayFile, DumpHandler};
use super::{Source, Module, ModuleCommon, UdpReader};

const TIME_BASE: i64 = 100; // ns
const MAX_PACKET_SIZE: usize = 2048;
const HEADER_LEN: usize = 42;
const EVENT_SIZE: usize = 6;
const BEG_MARKER: &[u8] = b"\x00\x00\x55\x55\xaa\xaa\xff\xff";
const PKT_MARKER: &[u8] = b"\x00\x00\xff\xff\x55\x55\xaa\xaa";
const END_MARKER: &[u8] = b"\xff\xff\xaa\xaa\x55\x55\x00\x00";
const FILE_START: &[u8] = b"mesytec ";
const FULL_HEADER: &[u8] = b"mesytec psd listmode data\nheader length: 2 lines \n";

pub struct MesyModule<S, C> {
    source: S,
    command_handler: C,
    module: ModuleId,
    dump: DumpHandler,
    is_master: bool,
}

impl MesyModule<(), ()> {
    pub fn start(config: MesyConfig, common: ModuleCommon) -> UResult<()> {
        match &config.local {
            SourceConfig::IP(addr) => {
                let reader = UdpReader::from_config(addr)?;
                let local = reader.0.local_addr().context("Getting local address of UDP reader")?;
                let cmds = cmd::make_command_socket(local, &config)?;
                MesyModule::start_with_source(reader, cmds, config, common)
            }
            SourceConfig::File(path) => {
                MesyModule::start_with_source(ReplayFile::from_config(path)?, (), config, common)
            }
        }
    }
}

impl<S: MesySource, C: cmd::MesyCommandHandler> MesyModule<S, C> {
    fn start_with_source(source: S, mut commands: C, config: MesyConfig, common: ModuleCommon) -> UResult<()> {
        let modules = commands.scan()?;
        commands.set_up(&modules, &config)?;
        let module = Self {
            source,
            command_handler: commands,
            module: common.module,
            dump: Default::default(),
            is_master: config.is_master,
        };
        module.start_main_loop(common)?;
        Ok(())
    }
}

impl<S: MesySource, C: cmd::MesyCommandHandler> Module for MesyModule<S, C> {
    fn description(&self) -> String {
        format!("MCPD module {} at {}", self.module.0, self.source.description())
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        if let Command::SetRawDump { enable, path } = cmd {
            self.dump.configure(enable, path)?;
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        self.dump.start(self.module, &run_id)?;
        self.dump.write(FULL_HEADER)?;
        self.dump.write(BEG_MARKER)?;
        self.source.rewind()?;
        if self.is_master {
            self.command_handler.start()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.write(END_MARKER)?;
        self.dump.stop();
        if self.is_master {
            self.command_handler.stop()?;
        }
        Ok(())
    }

    fn reset(&mut self) -> UResult<()> {
        self.source.reset()?;
        // TODO self.command_handler.set_up()?;
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        let n = match self.source.get_packet(&mut buffer) {
            Ok(0) => return Ok(vec![]),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(vec![]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(UError::NoMoreData),
            Err(e) => Err(e).context("Reading packet from source")?,
        };
        self.dump.write(&buffer[..n])?;
        self.dump.write(PKT_MARKER)?;

        let btype = S::E::read_u16(&buffer[2..4]);
        if btype >> 15 != 0 {
            // not a data buffer
            lprintln!(WARN, "Mesy: got a nondata buffer?");
            return Ok(vec![]);
        }
        // TODO: check packet a bit more
        let nevents = (n - HEADER_LEN) / EVENT_SIZE;
        let id_status = S::E::read_u16(&buffer[10..12]);
        let status = id_status & 0xFF;
        let mcpd_id = id_status as u64 >> 8;
        let pkt_ts = read_48bit::<S::E>(&buffer[12..]);
        if status & 1 != 1 {
            lprintln!(WARN, "Mesy: got event buffer but daq stopped?");
        }

        let mut events = Vec::with_capacity(nevents);
        for i in 0..nevents {
            let data = read_48bit::<S::E>(&buffer[HEADER_LEN + i*EVENT_SIZE..]);
            let ts = pkt_ts + (data & 0x7ffff);    // 19bit
            let event = if data >> 47 == 1 {
                // trigger event
                let data_id = (data >> 40) & 0b1111;
                Event::new(
                    EventTime::from_ticks(TIME_BASE, ts as i64),
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
                // Most general setup, needs correction for MPSD.
                let xpos = mcpd_id << 7 | mod_id << 4 | slot_id;

                Event::new(
                    EventTime::from_ticks(TIME_BASE, ts as i64),
                    EventTime::zero(),
                    self.module,
                    InputId(xpos as u32),
                    EventFlags::None,
                    EventData::RawDigital { value1: ypos as u32, value2: ampl as u32, value3: 0 }
                )
            };
            events.push(event);
        }
        events.sort();

        Ok(events)
    }
}

/// Read an 48-bit value (header timestamp or event) from the buffer.
///
/// Endianness of individual words can change, but the least significant
/// word is always first.
fn read_48bit<E: ByteOrder>(buf: &[u8]) -> u64 {
    let s1 = E::read_u16(&buf[0..2]) as u64;
    let s2 = E::read_u16(&buf[2..4]) as u64;
    let s3 = E::read_u16(&buf[4..6]) as u64;
    s3 << 32 | s2 << 16 | s1
}


/// Abstraction for reading Mesytec event packets from either the network
/// or a dump file.
///
/// In the dump file, additional markers are present, and the file header
/// must be skipped.
pub trait MesySource: Source {
    /// The Mesytec on-wire data format is little-endian while the dump file format
    /// is big-endian.  There doesn't appear to be a good reason for this.
    type E: ByteOrder;

    /// Read one packet into the provided buffer, returning the number of bytes
    /// read.
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

impl MesySource for UdpReader {
    type E = LE;

    /// On the wire, every packet just comes in as a datagram.
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let n = self.0.recv(buffer)?;
        // Consistency check the packet header.
        let packet_n = 2 * LE::read_u16(&buffer[..2]) as usize;
        if packet_n != n {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                                      "Packet too large for buffer or invalid packet"));
        }
        Ok(n)
    }
}

impl MesySource for ReplayFile {
    type E = BE;

    /// The Mesytec listmode data format is structured like this:
    ///
    /// - File header: some lines of ASCII text (usually 2)
    /// - 8-byte "beginning marker"
    /// - Packets, every one followed by 8-byte "packet end marker"
    /// - 8-byte "end marker"
    ///
    /// Here, we read every packet including the following end marker.
    fn get_packet(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let head = &mut buffer[..8];
        self.read_exact(head)?;
        if head == FILE_START {
            let mut linebreaks = 0;
            loop {
                let mut header_lines = 2;
                let mut byte = [0_u8; 1];
                let mut buffer = Vec::new();
                self.read_exact(&mut byte)?;
                buffer.push(byte[0]);
                if byte[0] == b'\n' {
                    linebreaks += 1;

                    if linebreaks == 2 {
                        // this line should contain the number of header lines
                        if let Some(pos) = buffer.windows(15).position(|w| w == b"header length: ") {
                            let start_num = pos + 15;
                            if let Some(end_num) = buffer[start_num..].windows(6).position(|w| w == b" lines")
                                && let Some(num) = parse_int(&buffer[start_num..][..end_num])
                            {
                                header_lines = num;
                            }
                        }
                    }

                    if linebreaks == header_lines {
                        break;
                    }
                }
            }
            // read, check and skip the begin marker
            self.read_exact(head)?;
            if head != BEG_MARKER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid file header"));
            }
            Ok(0)
        } else if head == END_MARKER {
            // nothing more to read here
            Ok(0)
        } else {
            let n = 2 * BE::read_u16(&buffer[..2]) as usize;
            if n > buffer.len() || n < HEADER_LEN {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                                          "Packet size too small or too large"));
            }
            self.read_exact(&mut buffer[8..n])?;
            // read, check and skip the packet end marker
            let mut pkt_end = [0; 8];
            self.read_exact(&mut pkt_end)?;
            if pkt_end != PKT_MARKER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid packet end marker"));
            }
            Ok(n)
        }
    }
}

fn parse_int(s: &[u8]) -> Option<u64> {
    str::from_utf8(s).ok()?.trim().parse::<u64>().ok()
}
