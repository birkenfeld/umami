// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fmt::Debug;
use std::mem::size_of;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use anyhow::{anyhow, Context};
use byteorder::{ByteOrder, LE};
use num_enum::FromPrimitive;
use zerocopy::{FromBytes, Immutable, IntoBytes, Unaligned};
use zerocopy::byteorder::little_endian::U16;
use crate::{ldebug, lprintln};
use crate::config::{MesyCellConfig, MesyConfig, MesyModuleConfig, SourceConfig};
use crate::error::UResult;
use crate::util::resolve;

const HEADER_WORDS: u16 = 10;
const BUFFERTYPE: u16 = 0x8000;
const ERROR_FLAG: u16 = 0x8000;

// Transmission mode bits (capability/mode register): which fields the
// event stream carries. Not cumulative -- a module/MCPD capability
// register is a bitmask of which of these are individually supported.
const TX_P: u16 = 1;   // position only
const TX_TP: u16 = 2;  // time + position
const TX_TPA: u16 = 4; // time + position + amplitude

#[repr(u16)]
#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Reset = 0,
    Start = 1,
    Stop = 2,
    SetCommPars = 5,
    SetTiming = 6,
    SetCell = 9,
    SetGainMpsd = 13,
    SetThreshold = 14,
    GetCapabilities = 22,
    SetCapabilities = 23,
    GetModInfo = 24,
    ReadIds = 36,
    GetMcpdVer = 51,
    // ReadPeriReg = 52,
    WritePeriReg = 53,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, FromPrimitive)]
pub enum ModType {
    None = 0,
    Mpsd8Old = 1,
    Mcpd8 = 2,
    Mdll = 35,
    Mpsd8SADC = 102,
    Mpsd8 = 103,
    Mstd16 = 104,
    Mpsd8P = 105,
    Mwpchr = 110,  // Multi wire proportional chamber high resolution (DEL-FRMII)
    Mstd16P = 204, // MSTD-16+, not official, but for easier handling of different MSTD-16 modules
    MdllP = 235,   // MDLL+, not official, but for easier handling of different MDLL modules
    #[num_enum(default)]
    Unknown,
}

/// Abstraction for the Mesytec command handler.
///
/// When reading from a dump file, no commands can be sent (or are required).
pub trait MesyCommandHandler: Send + 'static {
    /// Send a command with given ID and data, expecting a return data of given length.
    fn do_command<Din, Dout>(&mut self, cmd: Cmd, data: Din) -> UResult<Dout>
        where Din: IntoBytes + Immutable + Unaligned + Debug, Dout: FromBytes + Debug;

    fn do_noarg_cmd(&mut self, cmd: Cmd) -> UResult<()> {
        self.do_command(cmd, ())
    }

    fn start(&mut self) -> UResult<()> {
        self.do_noarg_cmd(Cmd::Reset)?;
        self.do_noarg_cmd(Cmd::Start)?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.do_noarg_cmd(Cmd::Stop)?;
        Ok(())
    }

    /// Returns the module type and transmission-mode capability bitmask
    /// (see [`Self::set_tx_mode`]) of each of the 8 module slots.
    fn scan(&mut self) -> UResult<([ModType; 8], [u16; 8])> {
        let mcpd_ver: [U16; 3] = self.do_command(Cmd::GetMcpdVer, ())?;
        let cpu_major = mcpd_ver[0];
        let cpu_minor = mcpd_ver[1];
        let fpga_major = mcpd_ver[2] >> 8;
        let fpga_minor = mcpd_ver[2] & 0xFF;
        lprintln!(INFO, "MCPD version: CPU {}.{} FPGA {}.{}",
                  cpu_major, cpu_minor, fpga_major, fpga_minor);

        let ids: [U16; 8] = self.do_command(Cmd::ReadIds, U16::new(2))?;
        let mut mod_types = [ModType::None; 8];
        let mut mod_xmit_caps = [0u16; 8];

        for (i, mod_id) in ids.into_iter().enumerate() {
            mod_types[i] = ModType::from(mod_id.get());
            if mod_id == 0 {
                continue;
            }

            let mod_info: [U16; 4] = self.do_command(Cmd::GetModInfo, [U16::new(i as _)])?;
            let mod_xmit_cap = mod_info[1];
            let mod_xmit_set = mod_info[2];
            let mod_major = mod_info[3] >> 8;
            let mod_minor = mod_info[3] & 0xFF;
            mod_xmit_caps[i] = mod_xmit_cap.get();

            lprintln!(INFO, "MCPD module {i}: ID {}, xmit cap {}, xmit set {}, firmware {}.{}",
                      mod_id, mod_xmit_cap, mod_xmit_set, mod_major, mod_minor);
        }
        Ok((mod_types, mod_xmit_caps))
    }

    fn set_up(&mut self, modules: &[ModType; 8], mod_xmit_caps: &[u16; 8], config: &MesyConfig) -> UResult<()> {
        if let SourceConfig::IP(addr) = &config.local {
            let data_port = resolve(addr)?.port();
            let _: [U16; 14] = self.do_command(Cmd::SetCommPars, [
                U16::new(0), U16::new(0), U16::new(0), U16::new(0),  // no new mcpd ip
                U16::new(0), U16::new(0), U16::new(0), U16::new(0),  // data ip = self
                U16::new(0),  // no new cmd port
                U16::new(data_port),
                U16::new(0), U16::new(0), U16::new(0), U16::new(0),  // cmd ip = self
            ])?;
            lprintln!(INFO, "Set target data port to {data_port}");
        }

        self.set_timing(config.is_master, config.terminate, config.ext_sync)?;
        self.set_tx_mode(modules, mod_xmit_caps, config.transmit_ampl)?;

        for i in 0..8 {
            if let Some(cfg) = config.cells.get(&i) {
                self.set_up_cell(i, cfg)?;
            }
        }

        for (i, modtype) in modules.iter().enumerate() {
            if *modtype != ModType::None {
                self.set_up_module(i, *modtype, config.modules.get(&i))?;
            }
        }
        Ok(())
    }

    /// Push this MCPD's sync-bus master/slave role, termination, and external
    /// sync setting to hardware. A master is always terminated regardless of
    /// `terminate`, and `ext_sync` is only meaningful when `master` is set.
    fn set_timing(&mut self, master: bool, terminate: bool, ext_sync: bool) -> UResult<()> {
        lprintln!(INFO, "Setting timing setup: master {master}, terminate {terminate}, ext_sync {ext_sync}");
        let term = master || terminate;
        let data0 = master as u16 + 2 * ext_sync as u16;
        let _res: [U16; 2] = self.do_command(Cmd::SetTiming, [U16::new(data0), U16::new(term as u16)])?;
        Ok(())
    }

    /// Negotiate and push the transmission mode: the richest of P (position
    /// only) / TP (+time) / TPA (+amplitude) supported by both the MCPD and
    /// every present module, capped at TP if `transmit_ampl` is false
    /// (amplitude data adds per-event processing overhead that can matter
    /// at high count rates). Pushed both MCPD-wide and to each module.
    fn set_tx_mode(&mut self, modules: &[ModType; 8], mod_xmit_caps: &[u16; 8],
                   transmit_ampl: bool) -> UResult<()> {
        let cap_reply: [U16; 2] = self.do_command(Cmd::GetCapabilities, ())?;
        let mut cap = cap_reply[0].get();
        for (i, modtype) in modules.iter().enumerate() {
            if *modtype != ModType::None {
                cap &= mod_xmit_caps[i];
            }
        }
        if !transmit_ampl {
            cap &= TX_P | TX_TP;
        }
        let mode = if cap & TX_TPA != 0 {
            TX_TPA
        } else if cap & TX_TP != 0 {
            TX_TP
        } else {
            TX_P
        };
        lprintln!(INFO, "Setting transmission mode to {mode} (common capability {cap})");
        let _res: [U16; 1] = self.do_command(Cmd::SetCapabilities, [U16::new(mode)])?;

        // Update each module's own transmission mode too (peripheral register 1).
        for (i, modtype) in modules.iter().enumerate() {
            if *modtype != ModType::None {
                let _res: [U16; 3] = self.do_command(
                    Cmd::WritePeriReg,
                    [U16::new(i as _), U16::new(1), U16::new(mode)],
                )?;
            }
        }
        Ok(())
    }

    /// Push one cell's trigger source/compare wiring to hardware. Also used
    /// to apply a live `SetParams` update to a running input.
    fn set_up_cell(&mut self, idx: usize, cfg: &MesyCellConfig) -> UResult<()> {
        lprintln!(INFO, "Setting up cell {idx} with source {}, compare {}",
                  cfg.source, cfg.compare);
        let _res: [U16; 3] = self.do_command(
            Cmd::SetCell,
            [U16::new(idx as _), U16::new(cfg.source), U16::new(cfg.compare)],
        )?;
        Ok(())
    }

    /// Push one module's threshold/gain to hardware, if `modtype` is a
    /// configurable MPSD-class module. Also used to apply a live
    /// `SetParams` update to a running input.
    fn set_up_module(&mut self, idx: usize, modtype: ModType,
                     cfg: Option<&MesyModuleConfig>) -> UResult<()> {
        match modtype {
            ModType::Mpsd8SADC | ModType::Mpsd8 | ModType::Mpsd8P => match cfg {
                Some(MesyModuleConfig::Mpsd { threshold, gain }) =>
                    self.set_up_mpsd(idx, *threshold, *gain),
                Some(_) => {
                    lprintln!(WARN, "Module {idx} is not an MPSD, not configuring");
                    Ok(())
                }
                None => {
                    lprintln!(WARN, "MPSD {idx} has no assigned config, not configuring");
                    Ok(())
                }
            },
            ModType::Mwpchr => {
                lprintln!(INFO, "Module {idx} is a MWPCHR, no configuration necessary");
                Ok(())
            }
            other => {
                lprintln!(WARN, "Module {idx} has unsupported type {other:?}, not configuring");
                Ok(())
            }
        }
    }

    fn set_up_mpsd(&mut self, num: usize, threshold: u16, gain: u16) -> UResult<()> {
        lprintln!(INFO, "Setting up MPSD {num} with threshold {threshold}, gain {gain}");
        let _res: [U16; 3] = self.do_command(
            Cmd::SetGainMpsd,
            [U16::new(num as _), U16::new(8), U16::new(gain)],
        )?;   // id 8 = all channels (TODO single gain)
        let _res: [U16; 2] = self.do_command(
            Cmd::SetThreshold,
            [U16::new(num as _), U16::new(threshold)],
        )?;
        Ok(())
    }
}

impl MesyCommandHandler for () {
    fn do_command<Din, Dout>(&mut self, _cmd: Cmd, _data: Din) -> UResult<Dout>
        where Din: IntoBytes + Immutable + Unaligned + Debug, Dout: FromBytes + Debug
    {
        Ok(Dout::new_zeroed())
    }
}

pub struct CommandSocket {
    sock: UdpSocket,
    buffer: [u8; 2048],
    mcpd_id: u8,
    buf_count: u16,
}

#[repr(C)]
#[derive(Debug, Default, FromBytes, IntoBytes, Immutable, Unaligned)]
struct Header {
    // does not include the 0xFFFF terminator
    buf_len: U16,
    buf_type: U16,
    hdr_len: U16,
    serial: U16,
    cmd_id: U16,
    status: u8,
    mcpd_id: u8,
    ts_1: U16,
    ts_2: U16,
    ts_3: U16,
    checksum: U16,
}

#[repr(C)]
#[derive(Debug, FromBytes, IntoBytes, Immutable)]
struct Packet<Data> {
    hdr: Header,
    data: Data,
    trailer: U16,
}

impl MesyCommandHandler for CommandSocket {
    fn do_command<Din, Dout>(&mut self, cmd: Cmd, data: Din) -> UResult<Dout>
        where Din: IntoBytes + Immutable + Unaligned + Debug, Dout: FromBytes + Debug
    {
        let cmd_id = cmd as u16;
        let serial = self.buf_count;
        self.buf_count = self.buf_count.wrapping_add(1);

        // assemble packet
        let mut packet = Packet {
            hdr: Header {
                buf_type: BUFFERTYPE.into(),
                hdr_len: HEADER_WORDS.into(),
                buf_len: (HEADER_WORDS + size_of::<Din>() as u16/2).into(),
                serial: serial.into(),
                cmd_id: cmd_id.into(),
                mcpd_id: self.mcpd_id,
                .. Default::default()
            },
            data,
            trailer: 0.into(),
        };
        let chksum = packet.as_bytes().chunks(2)
                                      .fold(0, |sum, chunk| sum ^ LE::read_u16(chunk));
        packet.hdr.checksum.set(chksum);
        packet.trailer.set(0xFFFF);
        ldebug!("Mesytec command: {packet:?}");
        // ldebug!("Mesytec command buffer: {:?}", packet.as_bytes());

        // exchange communication
        self.sock.send(packet.as_bytes())
                 .with_context(|| format!("Sending command {cmd:?} to command socket"))?;
        let mut nrecv = self.sock.recv(&mut self.buffer)
                                 .with_context(|| format!("Receiving reply to command {cmd:?}"))?;
        // ldebug!("Mesytec reply buffer: {:?}", &self.buffer[..nrecv]);
        // some partners do not reply with trailing 0xFFFF
        if self.buffer[nrecv-2..nrecv] != [0xFF, 0xFF] {
            nrecv += 2;  // no need to set the bytes, we ignore them
        }

        let ret = Packet::<Dout>::read_from_prefix(&self.buffer[..nrecv])
            .map(|(pkt, _)| pkt)
            .map_err(|_| anyhow!("Reply packet has wrong length (expected {}, got {})",
                                 size_of::<Packet<Dout>>(), nrecv))
            .with_context(|| format!("Sending command {cmd:?}"))?;
        ldebug!("Mesytec reply: {ret:?}");

        // consistency checks
        if ret.hdr.buf_len != nrecv as u16 / 2 - 1 {
            return data_err("Reply packet has wrong length header", ret.hdr.buf_len, nrecv / 2 - 1);
        }
        if ret.hdr.buf_type != BUFFERTYPE {
            return data_err("Reply packet has wrong buffer type", ret.hdr.buf_type, BUFFERTYPE);
        }
        if ret.hdr.hdr_len != HEADER_WORDS {
            return data_err("Reply packet has wrong header length", ret.hdr.hdr_len, HEADER_WORDS);
        }
        if ret.hdr.serial != serial {
            return data_err("Reply packet has wrong buffer serial number", ret.hdr.serial, serial);
        }
        if ret.hdr.cmd_id != cmd_id {
            if ret.hdr.cmd_id == cmd_id | ERROR_FLAG {
                return data_err("Command failed", ret.hdr.cmd_id, cmd_id);
            }
            return data_err("Reply packet has wrong command ID", ret.hdr.cmd_id, cmd_id);
        }
        if ret.hdr.mcpd_id != self.mcpd_id {
            return data_err("Reply packet has wrong MCPD ID", ret.hdr.mcpd_id, self.mcpd_id);
        }

        Ok(ret.data)
    }
}

fn data_err<T>(msg: &str, got: impl ToString, exp: impl ToString) -> UResult<T> {
    Err(anyhow!("{} (expected {}, got {})", msg, exp.to_string(), got.to_string()))?
}

pub fn make_command_socket(local_data_addr: SocketAddr, config: &MesyConfig) -> UResult<CommandSocket> {
    let local_ip = local_data_addr.ip();
    let port = local_data_addr.port() + 1;
    let socket = UdpSocket::bind((local_ip, port))
        .with_context(|| format!("Binding UDP command socket to port {port}"))?;
    let cmd_addr = resolve(&config.remote)?;
    socket.connect(cmd_addr)
        .with_context(|| format!("Connnecting UDP command socket to {cmd_addr}"))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))
        .context("Setting receive timeout on UDP command socket")?;
    Ok(CommandSocket {
        sock: socket,
        buffer: [0; 2048],
        mcpd_id: config.mcpd_id,
        buf_count: 0,
    })
}
