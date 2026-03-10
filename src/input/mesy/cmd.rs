// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use byteorder::{ByteOrder, BE, WriteBytesExt};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use crate::lprintln;
use crate::config::MesyConfig;
use crate::error::UResult;
use crate::util::resolve;

const BUFFERTYPE: u16 = 0x8000;
const HEADERLEN:  usize = 20;  // in bytes

const RESET:         u16 = 0;
const START:         u16 = 1;
const STOP:          u16 = 2;
const GET_MOD_INFO:  u16 = 24;
const READ_IDS:      u16 = 36;
const GET_MCPD_VER:  u16 = 51;
const READ_PERI_REG: u16 = 52;

/// Abstraction for the Mesytec command handler.
///
/// When reading from a dump file, no commands can be sent (or are required).
pub trait MesyCommandHandler: Send + 'static {
    /// Send a command with given ID and data, expecting a return data of given length.
    fn do_command(&mut self, cmd_id: u16, data: &[u8], exp: usize) -> io::Result<&[u8]>;

    fn start(&mut self) -> UResult<()> {
        self.do_command(RESET, b"", 0).context("Sending RESET")?;
        self.do_command(START, b"", 0).context("Sending START")?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.do_command(STOP, b"", 0).context("Sending STOP")?;
        Ok(())
    }

    fn scan(&mut self) -> UResult<()> {
        let mcpd_ver = self.do_command(GET_MCPD_VER, b"", 6).context("Sending GET_MCPD_VER")?;
        let cpu_major = BE::read_u16(&mcpd_ver[0..2]);
        let cpu_minor = BE::read_u16(&mcpd_ver[2..4]);
        let fpga_major = mcpd_ver[4];
        let fpga_minor = mcpd_ver[5];
        lprintln!(INFO, "MCPD version: CPU {}.{} FPGA {}.{}",
                  cpu_major, cpu_minor, fpga_major, fpga_minor);

        let id_array = self.do_command(READ_IDS, &[0, 2], 16).context("Sending READ_IDS")?;
        let mut ids = [0u16; 8];
        BE::read_u16_into(id_array, &mut ids);

        for (i, mod_id) in ids.into_iter().enumerate() {
            if mod_id == 0 {
                continue;
            }
            let peri_reg = self.do_command(READ_PERI_REG, &[0, i as u8, 0, 2], 6)
                               .context("Sending READ_PERI_REG for version")?;
            let mod_ver = BE::read_u16(&peri_reg[4..6]);
            let mod_info = self.do_command(GET_MOD_INFO, &[0, i as u8], 8)
                               .context("Sending GET_MOD_INFO")?;
            let mod_xmit_cap = BE::read_u16(&mod_info[2..4]);
            let mod_xmit_set = BE::read_u16(&mod_info[4..6]);
            let mod_firmware = BE::read_u16(&mod_info[6..8]);

            lprintln!(INFO, "Module {}: ID {}, version {}, xmit cap {}, xmit set {}, firmware {}",
                      i, mod_id, mod_ver, mod_xmit_cap, mod_xmit_set, mod_firmware);
        }

        Ok(())
    }
}

impl MesyCommandHandler for () {
    fn do_command(&mut self, _cmd_id: u16, _data: &[u8], _exp: usize) -> io::Result<&[u8]> {
        Ok(&[])
    }
}

pub struct MesyCommandSocket {
    sock: UdpSocket,
    buffer: Vec<u8>,
    mcpd_id: u8,
    buf_count: u16,
}

impl MesyCommandHandler for MesyCommandSocket {
    fn do_command(&mut self, cmd_id: u16, data: &[u8], exp: usize) -> io::Result<&[u8]> {
        let serial = self.buf_count;
        self.buf_count = self.buf_count.wrapping_add(1);

        // assemble packet
        self.buffer.clear();
        self.buffer.write_u16::<BE>(BUFFERTYPE)?;
        self.buffer.write_u16::<BE>(2*HEADERLEN as u16)?;
        self.buffer.write_u16::<BE>(serial)?;
        self.buffer.write_u16::<BE>((HEADERLEN + data.len()) as u16 / 2)?;
        self.buffer.write_u16::<BE>(cmd_id)?;
        self.buffer.write_u8(self.mcpd_id)?;
        self.buffer.write_u8(0)?;  // status
        self.buffer.write_u32::<BE>(0)?;  // header timestamp
        self.buffer.write_u16::<BE>(0)?;
        self.buffer.write_u16::<BE>(0)?;  // checksum - later
        self.buffer.extend_from_slice(data);
        let chksum = data.chunks(2).fold(0, |sum, chunk| sum ^ BE::read_u16(chunk));
        BE::write_u16(&mut self.buffer[18..20], chksum);
        self.buffer.write_u16::<BE>(0xFFFF)?;  // end of packet marker

        // exchange communication
        self.sock.send(&self.buffer)?;
        self.buffer.resize(2048, 0);
        let n = self.sock.recv(&mut self.buffer)?;

        // validate reply
        if n < HEADERLEN {
            return data_err("Reply packet too short", n, HEADERLEN);
        }
        let pkt_len = BE::read_u16(&self.buffer[0..2]) as usize;
        let buf_type = BE::read_u16(&self.buffer[2..4]);
        let hdr_len = BE::read_u16(&self.buffer[4..6]) as usize;
        let ret_serial = BE::read_u16(&self.buffer[6..8]);
        let ret_cmd_id = BE::read_u16(&self.buffer[8..10]);
        let ret_mcpd_id = self.buffer[10];
        if pkt_len != (n - 2) / 2 {
            return data_err("Reply packet has wrong length", pkt_len, (n - 2) / 2);
        }
        if buf_type != BUFFERTYPE {
            return data_err("Reply packet has wrong buffer type", buf_type, BUFFERTYPE);
        }
        if hdr_len != HEADERLEN / 2 {
            return data_err("Reply packet has wrong header length", hdr_len, HEADERLEN / 2);
        }
        if ret_serial != serial {
            return data_err("Reply packet has wrong buffer serial number", ret_serial, serial);
        }
        if ret_cmd_id != cmd_id {
            if ret_cmd_id == cmd_id | 0x8000 {
                return data_err("Command failed", ret_cmd_id, cmd_id);
            } else {
                return data_err("Reply packet has wrong command ID", ret_cmd_id, cmd_id);
            }
        }
        if ret_mcpd_id != self.mcpd_id {
            return data_err("Reply packet has wrong MCPD ID", ret_mcpd_id, self.mcpd_id);
        }
        if n - HEADERLEN - 2 != exp {
            return data_err("Reply packet has wrong data length", n - 22, exp);
        }

        Ok(&self.buffer[HEADERLEN..n-2])
    }
}

pub fn make_command_socket(local_data_addr: SocketAddr, config: &MesyConfig) -> UResult<MesyCommandSocket> {
    let local_ip = local_data_addr.ip();
    let port = local_data_addr.port() + 1;
    let socket = UdpSocket::bind((local_ip, port))
        .with_context(|| format!("Binding UDP command socket to port {}", port))?;
    let cmd_addr = resolve(&config.remote)?;
    socket.connect(cmd_addr)
        .with_context(|| format!("Connnecting UDP command socket to {}", cmd_addr))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))
        .context("Setting receive timeout on UDP command socket")?;
    Ok(MesyCommandSocket {
        sock: socket,
        buffer: Vec::with_capacity(2048),
        mcpd_id: config.mcpd_id,
        buf_count: 0,
    })
}

fn data_err<T>(msg: &str, got: impl ToString, exp: impl ToString) -> Result<T, io::Error> {
    let msg = format!("{} (expected {}, got {})", msg, exp.to_string(), got.to_string());
    Err(io::Error::new(io::ErrorKind::InvalidData, msg))
}
