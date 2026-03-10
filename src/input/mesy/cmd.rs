// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use byteorder::{ByteOrder, BE, WriteBytesExt};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use crate::config::MesyConfig;
use crate::error::UResult;
use crate::util::resolve;

const BUFFERTYPE: u16 = 0x8000;
const HEADERLEN:  usize = 20;  // in bytes

const RESET:    u16 = 0;
const START:    u16 = 1;
const STOP:     u16 = 2;

/// Abstraction for the Mesytec command handler.
///
/// When reading from a dump file, no commands can be sent (or are required).
pub trait MesyCommandHandler: Send + 'static {
    fn do_command(&mut self, cmd_id: u16, data: &[u8]) -> io::Result<&[u8]>;

    fn start(&mut self) -> UResult<()> {
        self.do_command(RESET, b"").context("Sending RESET")?;
        self.do_command(START, b"").context("Sending START")?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.do_command(STOP, b"").context("Sending STOP")?;
        Ok(())
    }
}

impl MesyCommandHandler for () {
    fn do_command(&mut self, _cmd_id: u16, _data: &[u8]) -> io::Result<&[u8]> {
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
    fn do_command(&mut self, cmd_id: u16, data: &[u8]) -> io::Result<&[u8]> {
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
        if n < HEADERLEN*2 {
            return data_err("Reply packet too short");
        }
        if BE::read_u16(&self.buffer[0..2]) as usize != (n - 2) / 2 {
            return data_err("Reply packet has wrong length");
        }
        if BE::read_u16(&self.buffer[2..4]) != BUFFERTYPE {
            return data_err("Reply packet has wrong buffer type");
        }
        if BE::read_u16(&self.buffer[4..6]) as usize != HEADERLEN / 2 {
            return data_err("Reply packet has wrong header length");
        }
        if BE::read_u16(&self.buffer[6..8]) != serial {
            return data_err("Reply packet has wrong buffer serial number");
        }
        if BE::read_u16(&self.buffer[8..10]) != cmd_id {
            if BE::read_u16(&self.buffer[8..10]) == cmd_id | 0x8000 {
                return data_err("Command failed (reply has error bit set)");
            } else {
                return data_err("Reply packet has wrong command ID");
            }
        }
        if self.buffer[10] != self.mcpd_id {
            return data_err("Reply packet has wrong MCPD ID");
        }

        Ok(&self.buffer[20..n-2])
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

fn data_err<T>(msg: &str) -> Result<T, io::Error> {
    Err(io::Error::new(io::ErrorKind::InvalidData, msg))
}
