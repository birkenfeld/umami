// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod cmd;

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use anyhow::{anyhow, Context};
use byteorder::{ByteOrder, BE, LE};
use serde::{Deserialize, Serialize};
use crate::lprintln;
use crate::command::{Command, CommandReply, ModuleId};
use crate::config::SourceConfig;
use crate::error::{UError, UResult};
use crate::event::{Event, EventType, EventTime};
use crate::input::{ReplayFile, DumpHandler};
use crate::params::HasParams;
use super::{Source, Input, InputCommon, UdpReader};

const TIME_BASE: i64 = 100; // ns
const MAX_PACKET_SIZE: usize = 2048;
const HEADER_LEN: usize = 42;
const EVENT_SIZE: usize = 6;
const BEG_MARKER: &[u8] = b"\x00\x00\x55\x55\xaa\xaa\xff\xff";
const PKT_MARKER: &[u8] = b"\x00\x00\xff\xff\x55\x55\xaa\xaa";
const END_MARKER: &[u8] = b"\xff\xff\xaa\xaa\x55\x55\x00\x00";
const FILE_START: &[u8] = b"mesytec ";
const FULL_HEADER: &[u8] = b"mesytec psd listmode data\nheader length: 2 lines \n";

fn default_true() -> bool {
    true
}

/// Deserializes a `BTreeMap<usize, T>` from a string-keyed map, as TOML (and
/// JSON) always represent it on the wire -- the `toml` crate can't
/// deserialize a non-string-keyed map directly.
fn deserialize_usize_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<usize, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let string_keyed: BTreeMap<String, T> = BTreeMap::deserialize(deserializer)?;
    string_keyed.into_iter()
        .map(|(k, v)| {
            k.parse::<usize>()
                .map(|idx| (idx, v))
                .map_err(|_| serde::de::Error::custom(
                    format!("Invalid index key {k:?}, expected a number")))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct MesyConfig {
    pub local: SourceConfig,
    pub remote: String,
    pub is_master: bool,
    /// Sync-bus termination. Forced on for the master.
    pub terminate: bool,
    /// External synchronisation input, only meaningful when `is_master`.
    #[serde(default)]
    pub ext_sync: bool,
    /// Negotiate amplitude data (TPA) into the transmission mode if
    /// supported, vs. capping at time+position (TP) for lower overhead.
    #[serde(default = "default_true")]
    pub transmit_ampl: bool,
    pub mcpd_id: u8,
    #[serde(deserialize_with = "deserialize_usize_map")]
    pub cells: BTreeMap<usize, MesyCellConfig>,
    #[serde(deserialize_with = "deserialize_usize_map")]
    pub modules: BTreeMap<usize, MesyModuleConfig>,
}

/// A cell's trigger source: which physical/logical signal counts into it.
#[repr(u16)]
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CellTrigger {
    None = 0,
    Aux1 = 1,
    Aux2 = 2,
    Aux3 = 3,
    Aux4 = 4,
    Digital1 = 5,
    Digital2 = 6,
    Compare = 7,
}

/// A bit index into the MCPD's compare/status register: 0-20 select one of
/// its 21 status bits, 21 is the counter-overflow pseudo-bit, 22 is the
/// rising-edge pseudo-bit. Only meaningful when a cell's `source` is
/// `CellTrigger::Compare`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CompareBit(u16);

impl CompareBit {
    pub fn new(value: u16) -> anyhow::Result<Self> {
        if value > 22 {
            Err(anyhow!("Compare bit must be 0-22, got {value}"))?;
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for CompareBit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de>
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MesyCellConfig {
    pub source: CellTrigger,
    pub compare: CompareBit,
}

/// An MPSD's gain, either the same for every channel or given per channel
/// (one value per tube).
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MesyGain {
    Uniform(u16),
    PerChannel([u16; 8]),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MesyModuleConfig {
    // TODO better types
    // TODO amp mode
    Mpsd { threshold: u16, gain: MesyGain },
    Mstd { threshold: u16, gain: u16 },
}

/// One module's test-pulse injection setting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PulserConfig {
    chan: u16,
    pos: cmd::PulserPos,
    amp: u16,
    on: bool,
}

#[derive(HasParams)]
#[params(kind = "input", type = "mesy")]
pub struct MesyInput<S, C>
where
    C: cmd::MesyCommandHandler,
{
    source: S,
    command_handler: C,
    dump: DumpHandler,
    // configuration
    name: ModuleId,
    #[param(readonly = true, datatype = "array of detected module types",
            help = "Module type detected per MCPD slot at startup")]
    mod_types: [cmd::ModType; 8],
    #[param(has_setter = true, datatype = "map of cell index to (source, compare)",
            help = "Per-cell trigger source/compare setting")]
    cells: BTreeMap<usize, MesyCellConfig>,
    #[param(has_setter = true,
            datatype = "map of module index to (type, threshold, gain: number or 8-array)",
            help = "Per-MPSD threshold/gain")]
    modules: BTreeMap<usize, MesyModuleConfig>,
    #[param(has_setter = true, runtime_only = true,
            datatype = "map of module index to (chan, pos, amp, on)",
            help = "Per-module test-pulse injection")]
    pulser: BTreeMap<usize, PulserConfig>,
    // run-time
    buf_serial: Option<u16>,
    no_event_buffers: usize,
}

impl MesyInput<(), ()> {
    pub fn start(config: MesyConfig, confdir: &Path, common: InputCommon) -> UResult<()> {
        match &config.local {
            SourceConfig::IP(addr) => {
                let reader = UdpReader::from_config(addr, confdir)?;
                let local = reader.0.local_addr().context("Getting local address of UDP reader")?;
                let cmds = cmd::make_command_socket(local, &config)?;
                MesyInput::start_with_source(reader, cmds, config, common)
            }
            SourceConfig::File(path) => {
                MesyInput::start_with_source(
                    ReplayFile::from_config(path, confdir)?, (), config, common)
            }
        }
    }
}

impl<S: MesySource, C: cmd::MesyCommandHandler> MesyInput<S, C> {
    fn start_with_source(source: S, mut commands: C, config: MesyConfig, common: InputCommon) -> UResult<()> {
        let (mod_types, mod_xmit_caps) = commands.scan()?;
        commands.set_up(&mod_types, &mod_xmit_caps, &config)?;
        let input = Self {
            source,
            command_handler: commands,
            dump: Default::default(),
            name: common.name,
            mod_types,
            cells: config.cells,
            modules: config.modules,
            pulser: BTreeMap::new(),
            buf_serial: None,
            no_event_buffers: 0,
        };
        input.start_main_loop(common)?;
        Ok(())
    }
}

// Bounded only on C (not S), matching the struct's own `where` clause, so
// these are also callable from the HasParams impl the derive generates.
impl<S, C: cmd::MesyCommandHandler> MesyInput<S, C> {
    fn set_cells(&mut self, cells: BTreeMap<usize, MesyCellConfig>) -> UResult<()> {
        for (&idx, cfg) in &cells {
            self.command_handler.set_up_cell(idx, cfg)?;
        }
        self.cells = cells;
        Ok(())
    }

    fn set_modules(&mut self, modules: BTreeMap<usize, MesyModuleConfig>) -> UResult<()> {
        for (&idx, cfg) in &modules {
            let modtype = self.mod_types.get(idx).copied().unwrap_or(cmd::ModType::None);
            self.command_handler.set_up_module(idx, modtype, Some(cfg))?;
        }
        self.modules = modules;
        Ok(())
    }

    fn set_pulser(&mut self, pulser: BTreeMap<usize, PulserConfig>) -> UResult<()> {
        for (&idx, cfg) in &pulser {
            self.command_handler.set_pulser(idx, cfg.chan, cfg.pos, cfg.amp, cfg.on)?;
        }
        self.pulser = pulser;
        Ok(())
    }
}

impl<S: MesySource, C: cmd::MesyCommandHandler> Input for MesyInput<S, C> {
    fn description(&self) -> String {
        format!("MCPD {} at {}", self.name, self.source.description())
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        if let Command::SetRawDump { enable, path } = cmd {
            self.dump.configure(enable, path)?;
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        self.dump.start(self.name, &run_id)?;
        self.dump.write(FULL_HEADER)?;
        self.dump.write(BEG_MARKER)?;
        self.source.rewind()?;
        self.buf_serial = None;
        // TODO: check why this is needed on non-master modules
        self.command_handler.start()?;
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        self.dump.write(END_MARKER)?;
        self.dump.stop();
        self.command_handler.stop()?;
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
        // TODO: byte swap
        self.dump.write(&buffer[..n])?;
        self.dump.write(PKT_MARKER)?;

        let buf_length = S::E::read_u16(&buffer) as usize * 2;
        if buf_length != n {
            lprintln!(WARN, [self.name] "Got packet of size {n}, expected {buf_length}");
            return Ok(vec![]);
        }
        let btype = S::E::read_u16(&buffer[2..4]);
        if btype >> 15 != 0 {
            // not a data buffer
            lprintln!(WARN, [self.name] "Got an unexpected command buffer");
            return Ok(vec![]);
        }

        let nevents = (n - HEADER_LEN) / EVENT_SIZE;
        let buf_serial = S::E::read_u16(&buffer[6..]);
        let id_status = S::E::read_u16(&buffer[10..]);
        let status = id_status & 0xFF;
        let mcpd_id = u64::from(id_status) >> 8;
        let pkt_ts = read_48bit::<S::E>(&buffer[12..]);
        if status & 1 != 1 {
            lprintln!(WARN, [self.name] "Got event buffer but daq stopped");
            return Ok(vec![]);
        }
        if let Some(prev) = self.buf_serial && buf_serial != prev.wrapping_add(1) {
            lprintln!(WARN, [self.name] "Skipped {} buffer(s): serial {} -> {}",
                      buf_serial.wrapping_sub(prev + 1), prev, buf_serial);
        }
        self.buf_serial = Some(buf_serial);
        //lprintln!(DEBUG, [self.name] "Got a data buffer");

        let mut events = Vec::with_capacity(nevents);

        // no events within 250ms -> generate a heartbeat
        if nevents == 0 {
            self.no_event_buffers += 1;
            if self.no_event_buffers == 10 {
                events.push(Event::new(EventType::Heartbeat)
                            .with_abs_time(EventTime::from_ticks(TIME_BASE, pkt_ts as i64)));
                self.no_event_buffers = 0;
            }
        } else {
            self.no_event_buffers = 0;
        }

        for i in 0..nevents {
            let data = read_48bit::<S::E>(&buffer[HEADER_LEN + i*EVENT_SIZE..]);
            let ts = pkt_ts + (data & 0x7ffff);    // 19bit
            let event = if data >> 47 == 1 {
                // trigger event
                let data_id = (data >> 40) & 0b1111;
                Event::new(EventType::Edge { up: true })
                    .with_channel(data_id as u32)
                    .with_abs_time(EventTime::from_ticks(TIME_BASE, ts as i64))
            } else {
                // neutron event
                let mod_id = (data >> 44) & 0b111;
                let slot_id = (data >> 39) & 0b11111;

                let ampl = (data >> 29) & 0x3ff;
                let ypos = (data >> 19) & 0x3ff;
                // Most general setup, needs correction for MPSD.
                let xpos = mcpd_id << 7 | mod_id << 4 | slot_id;

                Event::new(EventType::Neutron)
                    .with_channel(xpos as u32)
                    .with_abs_time(EventTime::from_ticks(TIME_BASE, ts as i64))
                    .with_ampl(ampl as u32)
                    .with_raw(ypos as u32, 0)
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
    let s1 = u64::from(E::read_u16(&buf[0..2]));
    let s2 = u64::from(E::read_u16(&buf[2..4]));
    let s3 = u64::from(E::read_u16(&buf[4..6]));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamMap;

    #[test]
    fn test_mesy_config_parses_populated_cells_and_modules_from_toml() {
        let cfg: MesyConfig = toml::from_str(r#"
            local = "localhost:50000"
            remote = "localhost:50001"
            is_master = true
            terminate = true
            mcpd_id = 0

            [cells.1]
            source = "aux2"
            compare = 5

            [modules.3]
            type = "mpsd"
            threshold = 42
            gain = 7
        "#).unwrap();
        assert!(matches!(cfg.cells[&1].source, CellTrigger::Aux2));
        assert_eq!(cfg.cells[&1].compare.get(), 5);
        assert!(matches!(cfg.modules[&3],
                MesyModuleConfig::Mpsd { threshold: 42, gain: MesyGain::Uniform(7) }));
    }

    struct NoSource;

    impl Source for NoSource {
        type Config = ();
        fn from_config(_: &(), _: &Path) -> UResult<Self> { Ok(NoSource) }
        fn description(&self) -> String { "none".into() }
        fn read_exact(&mut self, _buf: &mut [u8]) -> io::Result<()> { unreachable!() }
        fn reset(&mut self) -> UResult<()> { unreachable!() }
    }

    impl MesySource for NoSource {
        type E = LE;
        fn get_packet(&mut self, _buffer: &mut [u8]) -> io::Result<usize> { unreachable!() }
    }

    fn make_input() -> MesyInput<NoSource, ()> {
        MesyInput {
            source: NoSource,
            command_handler: (),
            dump: Default::default(),
            name: ModuleId::new("mesy".into()),
            mod_types: [cmd::ModType::Mpsd8; 8],
            cells: BTreeMap::new(),
            modules: BTreeMap::new(),
            pulser: BTreeMap::new(),
            buf_serial: None,
            no_event_buffers: 0,
        }
    }

    #[test]
    fn test_get_params_reports_empty_cells_and_modules() {
        let input = make_input();
        let params = input.get_params(false).unwrap();
        assert!(params["cells"]["value"].as_object().unwrap().is_empty());
        assert!(params["modules"]["value"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_update_params_updates_cells_and_modules() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("modules".into(), serde_json::json!({
            "3": {"type": "mpsd", "threshold": 42, "gain": 7},
        }));
        set.insert("cells".into(), serde_json::json!({
            "1": {"source": "aux2", "compare": 5},
        }));
        input.update_params(ModuleId::new("mesy".into()), set).unwrap();

        assert!(matches!(input.modules[&3],
                MesyModuleConfig::Mpsd { threshold: 42, gain: MesyGain::Uniform(7) }));
        assert!(matches!(input.cells[&1].source, CellTrigger::Aux2));
        assert_eq!(input.cells[&1].compare.get(), 5);
    }

    #[test]
    fn test_update_params_rejects_out_of_range_compare_bit() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("cells".into(), serde_json::json!({
            "1": {"source": "compare", "compare": 23},
        }));
        assert!(input.update_params(ModuleId::new("mesy".into()), set).is_err());
    }

    #[test]
    fn test_update_params_updates_module_with_per_channel_gain() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("modules".into(), serde_json::json!({
            "3": {"type": "mpsd", "threshold": 42, "gain": [1, 2, 3, 4, 5, 6, 7, 8]},
        }));
        input.update_params(ModuleId::new("mesy".into()), set).unwrap();

        assert!(matches!(input.modules[&3],
                MesyModuleConfig::Mpsd { threshold: 42, gain: MesyGain::PerChannel(
                    [1, 2, 3, 4, 5, 6, 7, 8]) }));
    }

    #[test]
    fn test_get_params_reports_empty_pulser() {
        let input = make_input();
        let params = input.get_params(false).unwrap();
        assert!(params["pulser"]["value"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_update_params_updates_pulser() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("pulser".into(), serde_json::json!({
            "2": {"chan": 3, "pos": "middle", "amp": 60, "on": true},
        }));
        input.update_params(ModuleId::new("mesy".into()), set).unwrap();

        let cfg = &input.pulser[&2];
        assert_eq!(cfg.chan, 3);
        assert!(matches!(cfg.pos, cmd::PulserPos::Middle));
        assert_eq!(cfg.amp, 60);
        assert!(cfg.on);
    }

    #[test]
    fn test_get_params_reports_detected_mod_types_readonly() {
        let input = make_input();
        let params = input.get_params(false).unwrap();
        let types = params["mod_types"]["value"].as_array().unwrap();
        assert_eq!(types.len(), 8);
        assert!(types.iter().all(|t| t == "mpsd8"));
    }

    #[test]
    fn test_update_params_rejects_setting_mod_types() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("mod_types".into(), serde_json::json!(vec!["mpsd8"; 8]));
        assert!(input.update_params(ModuleId::new("mesy".into()), set).is_err());
    }

    #[test]
    fn test_update_params_wrong_module_type_is_only_a_warning() {
        let mut input = make_input();
        input.mod_types[2] = cmd::ModType::Mwpchr;
        let mut set = ParamMap::new();
        set.insert("modules".into(), serde_json::json!({
            "2": {"type": "mpsd", "threshold": 1, "gain": 1},
        }));
        // does not error even though module 2 isn't an MPSD
        input.update_params(ModuleId::new("mesy".into()), set).unwrap();
        assert!(matches!(input.modules[&2], MesyModuleConfig::Mpsd { .. }));
    }
}
