// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fmt::Debug;
use std::mem::size_of;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use anyhow::{anyhow, Context};
use byteorder::{ByteOrder, LE};
use zerocopy::{FromBytes, Immutable, IntoBytes, Unaligned};
use zerocopy::byteorder::little_endian::U16;
use crate::lprintln;
use crate::config::MesyConfig;
use crate::error::UResult;
use crate::util::resolve;

const HEADER_WORDS: u16 = 10;
const BUFFERTYPE: u16 = 0x8000;
const ERROR_FLAG: u16 = 0x8000;

#[repr(u16)]
#[derive(Clone, Copy, Debug)]
pub enum Cmd {
    Reset = 0,
    Start = 1,
    Stop = 2,
    GetModInfo = 24,
    ReadIds = 36,
    GetMcpdVer = 51,
    ReadPeriReg = 52,
}

/// Abstraction for the Mesytec command handler.
///
/// When reading from a dump file, no commands can be sent (or are required).
pub trait MesyCommandHandler: Send + 'static {
    /// Send a command with given ID and data, expecting a return data of given length.
    fn do_command<Din, Dout>(&mut self, cmd: Cmd, data: Din) -> UResult<Dout>
        where Din: IntoBytes + Immutable + Unaligned, Dout: FromBytes + Debug;

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

    fn scan(&mut self) -> UResult<()> {
        let mcpd_ver: [U16; 3] = self.do_command(Cmd::GetMcpdVer, ())?;
        let cpu_major = mcpd_ver[0];
        let cpu_minor = mcpd_ver[1];
        let fpga_major = mcpd_ver[2] >> 8;
        let fpga_minor = mcpd_ver[2] & 0xFF;
        lprintln!(INFO, "MCPD version: CPU {}.{} FPGA {}.{}",
                  cpu_major, cpu_minor, fpga_major, fpga_minor);

        let ids: [U16; 8] = self.do_command(Cmd::ReadIds, U16::new(2))?;

        for (i, mod_id) in ids.into_iter().enumerate() {
            if mod_id == 0 {
                continue;
            }
            let peri_reg: [U16; 3] = self.do_command(Cmd::ReadPeriReg,
                                                     [U16::new(i as _), U16::new(2)])?;
            let mod_ver = peri_reg[2];
            let mod_info: [U16; 4] = self.do_command(Cmd::GetModInfo, [U16::new(i as _)])?;
            let mod_xmit_cap = mod_info[1];
            let mod_xmit_set = mod_info[2];
            let mod_firmware = mod_info[3];

            lprintln!(INFO, "Module {}: ID {}, version {}, xmit cap {}, xmit set {}, firmware {}",
                      i, mod_id, mod_ver, mod_xmit_cap, mod_xmit_set, mod_firmware);
        }
        Ok(())
    }
}

impl MesyCommandHandler for () {
    fn do_command<Din, Dout>(&mut self, _cmd: Cmd, _data: Din) -> UResult<Dout>
        where Din: IntoBytes + Immutable, Dout: FromBytes + Debug
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
    buf_type: U16,
    hdr_len: U16,
    serial: U16,
    // does not include the 0xFFFF terminator
    buf_len: U16,
    cmd_id: U16,
    mcpd_id: u8,
    status: u8,
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
        where Din: IntoBytes + Immutable + Unaligned, Dout: FromBytes + Debug
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
        lprintln!(DEBUG, "Mesytec command buffer: {:?}", packet.as_bytes());

        // exchange communication
        self.sock.send(packet.as_bytes())
                 .with_context(|| format!("Sending command {:?} to command socket", cmd))?;
        let nrecv = self.sock.recv(&mut self.buffer)
                             .with_context(|| format!("Receiving reply to command {:?}", cmd))?;
        lprintln!(DEBUG, "Mesytec command reply: {:?}", &self.buffer[..nrecv]);

        let ret = Packet::<Dout>::read_from_bytes(&self.buffer[..nrecv])
            .map_err(|_| anyhow!("Reply packet has wrong length (expected {}, got {})",
                                 size_of::<Packet<Dout>>(), nrecv))
            .with_context(|| format!("Sending command {:?}", cmd))?;
        lprintln!(DEBUG, "Mesytec command reply: {:?}", ret);

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
            } else {
                return data_err("Reply packet has wrong command ID", ret.hdr.cmd_id, cmd_id);
            }
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
        .with_context(|| format!("Binding UDP command socket to port {}", port))?;
    let cmd_addr = resolve(&config.remote)?;
    socket.connect(cmd_addr)
        .with_context(|| format!("Connnecting UDP command socket to {}", cmd_addr))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))
        .context("Setting receive timeout on UDP command socket")?;
    Ok(CommandSocket {
        sock: socket,
        buffer: [0; 2048],
        mcpd_id: config.mcpd_id,
        buf_count: 0,
    })
}
