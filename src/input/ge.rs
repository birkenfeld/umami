// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::net::TcpStream;
use std::path::Path;
use anyhow::{anyhow, Context};
use byteorder::{ByteOrder, LE};
use serde::Deserialize;
use crate::command::{Command, CommandReply, ModuleId};
use crate::config::SourceConfig;
use crate::error::{UError, UResult};
use crate::input::{ReplayFile, DumpHandler};
use crate::event::{Event, EventTime, EventFlags, EventType};
use crate::params::HasParams;
use super::{Source, Input, InputCommon};

const PACKET_NORMAL:     u32 = 0x1000;
const PACKET_NORM_FAKE:  u32 = 0x1100;
const PACKET_DIAG:       u32 = 0x3000;
const PACKET_DIAG_FAKE:  u32 = 0x3100;
const PACKET_HEARTBT:    u32 = 0x5000;
const MAX_PACKET_SIZE: usize = 65536;

#[derive(Debug, Deserialize)]
pub struct GEConfig {
    pub source: SourceConfig,
    #[serde(default)]
    pub timestamper: bool,
}

#[derive(HasParams)]
#[params(kind = "input", type = "ge")]
pub struct GeInput<S>
where
    S: Source,
{
    source: S,
    name: ModuleId,
    dump: DumpHandler,
    queue: Vec<Event>,
    is_ts: bool,
    #[param(has_setter = true, runtime_only = true,
            help = "Path of the dump file currently being replayed (file-backed \
                    inputs only); switching it takes effect on the next Start")]
    replay_file: String,
}

fn read_time(buf: &[u8]) -> EventTime {
    let sec = LE::read_u32(&buf[0..4]);
    let nsec = LE::read_u32(&buf[4..8]);
    EventTime::from_sec_nsec(sec, nsec)
}

pub fn start(config: GEConfig, confdir: &Path, common: InputCommon) -> UResult<()> {
    match &config.source {
        SourceConfig::IP(addr) => GeInput::start_with_source(
            TcpStream::from_config(addr, confdir)?, config, common),
        SourceConfig::File(path) => GeInput::start_with_source(
            ReplayFile::from_config(path, confdir)?, config, common),
    }
}

impl<S: Source> GeInput<S> {
    fn start_with_source(source: S, config: GEConfig, common: InputCommon) -> UResult<()> {
        let replay_file = match &config.source {
            SourceConfig::File(path) => path.clone(),
            SourceConfig::IP(_) => "<live>".into(),
        };
        let input = Self {
            source,
            dump: Default::default(),
            name: common.name,
            queue: Vec::with_capacity(1024),
            is_ts: config.timestamper,
            replay_file,
        };
        input.start_main_loop(common)?;
        Ok(())
    }

    fn set_replay_file(&mut self, path: String) -> UResult<()> {
        self.source.switch_file(Path::new(&path))?;
        self.replay_file = path;
        Ok(())
    }
}

impl<S: Source> Input for GeInput<S> {
    fn description(&self) -> String {
        format!(
            "GE {} {} at {}",
            if self.is_ts { "TS" } else { "EP" },
            self.name,
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
        self.dump.start(self.name, &run_id)?;
        self.source.rewind()?;
        self.queue.clear();
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.stop();
        Ok(())
    }

    fn reset(&mut self) -> UResult<()> {
        self.source.reset()?;
        self.queue.clear();
        Ok(())
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        // read header
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        match self.source.read_exact(&mut buffer[..16]) {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(vec![]),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                if self.queue.is_empty() {
                    return Err(UError::NoMoreData);
                }
                self.queue.sort();
                return Ok(std::mem::take(&mut self.queue));
            }
            Err(e) => Err(e).context("Reading from source")?,
        }
        let len = LE::read_u32(&buffer) as usize;
        let pktype = LE::read_u32(&buffer[4..]);
        let header_time = read_time(&buffer[8..]);

        if len == 0 {
            if pktype == PACKET_HEARTBT {
                self.dump.write(&buffer[..16])?;
                return Ok(vec![Event::new(EventType::Heartbeat).with_abs_time(header_time)]);
            }
            return Err(anyhow!("Received empty packet of type {pktype:#x}").into());
        }

        // read the rest
        self.source.read_exact(&mut buffer[16..][..len])
                   .context("Reading packet content")?;
        let (evlen, flags) = match pktype {
            PACKET_NORMAL => (12, EventFlags::None),
            PACKET_NORM_FAKE => (12, EventFlags::Fake),
            PACKET_DIAG => (24, EventFlags::None),
            PACKET_DIAG_FAKE  => (24, EventFlags::Fake),
            _ => return Err(anyhow!("Unsupported packet type {pktype:#x}"))?,
        };
        let nevents = (len - 24) / evlen;
        let mut offset = 40;
        let mut events = Vec::with_capacity(self.queue.len() + nevents);

        for event in std::mem::replace(&mut self.queue, Vec::with_capacity(1024)) {
            if event.time < header_time {
                events.push(event);
            } else {
                self.queue.push(event);
            }
        }

        for _ in 0..nevents {
            let detid = LE::read_u32(&buffer[offset+8..]);
            let event = if self.is_ts {
                Event::new(EventType::Edge)
                    .with_index((detid >> 31) as u8)
                    .with_abs_time(read_time(&buffer[offset..]))
                    .with_channel(detid & 0xFFFF)
                    .with_flags(flags)
            } else if pktype == PACKET_DIAG || pktype == PACKET_DIAG_FAKE {
                // let max_heights = LE::read_u32(&buffer[offset+12..]);
                let a_integrated = LE::read_u32(&buffer[offset+16..]);
                let b_integrated = LE::read_u32(&buffer[offset+20..]);
                Event::new(EventType::Neutron)
                    .with_abs_time(read_time(&buffer[offset..]))
                    .with_channel(detid)
                    .with_flags(flags)
                    .with_raw(a_integrated, b_integrated)
                    .with_ampl(a_integrated + b_integrated)
            } else {
                Event::new(EventType::Neutron)
                    .with_abs_time(read_time(&buffer[offset..]))
                    .with_channel(detid)
                    .with_flags(flags)
            };

            // Use the packet header timestamp as a criterion - we know any events before
            // this timestamp have been sent in this or a previous packet.  Any events
            // afterwards can still have a later time than events coming in future packets.
            if event.time < header_time {
                events.push(event);
            } else {
                self.queue.push(event);
            }
            offset += evlen;
        }
        events.sort();

        self.dump.write(&buffer[..16+len])?;

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamMap;

    #[test]
    fn test_update_params_switches_replay_file_for_file_source() {
        let dir = std::env::temp_dir();
        let name = format!("umami_test_ge_replay_switch_{}.dat", std::process::id());
        let path = dir.join(&name);
        std::fs::write(&path, b"hello").unwrap();

        let mut input = GeInput {
            source: ReplayFile::from_config(&name, &dir).unwrap(),
            name: ModuleId::new("ge".into()),
            dump: Default::default(),
            queue: Vec::new(),
            is_ts: false,
            replay_file: name.clone(),
        };

        let other_name = format!("umami_test_ge_replay_switch_other_{}.dat", std::process::id());
        let other_path = dir.join(&other_name);
        std::fs::write(&other_path, b"world!").unwrap();

        let mut set = ParamMap::new();
        set.insert("replay_file".into(), serde_json::json!(other_path.to_str().unwrap()));
        input.update_params(ModuleId::new("ge".into()), set).unwrap();
        assert_eq!(input.replay_file, other_path.to_str().unwrap());

        let mut buffer = [0u8; 6];
        input.source.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"world!");

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&other_path).ok();
    }
}
