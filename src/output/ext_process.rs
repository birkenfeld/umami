// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::HashSet;
use std::io::Write;
use std::mem::replace;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use anyhow::{anyhow, Context};
use num_enum::{FromPrimitive, IntoPrimitive};
use serde::{Deserialize, Serialize};
use uds::{UnixListenerExt, UnixSocketAddr};
use zerocopy::IntoBytes;
use crate::channel::{self, Receiver, RecvTimeoutError, Sender};
use crate::lprintln;
use crate::command::ModuleId;
use crate::error::UResult;
use crate::event::{Event, EventType};
use crate::params::HasParams;
use super::{Output, OutputCommon};

/// Represents a frame of data to be sent to the external consumer.
enum Frame {
    Events(Vec<Event>),
    StartOfRun(String),
    EndOfRun,
    Clear,
}

/// Wire-format frame tag, see `docs/outputs.md`'s `ext_process` section.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, IntoPrimitive)]
pub enum FrameTag {
    Events = 0,
    StartOfRun = 1,
    EndOfRun = 2,
    Clear = 3,
    #[num_enum(default)]
    Unknown = 0xff,
}

/// Bounds how many frames may be queued for the write thread.
const FRAME_QUEUE_SIZE: usize = 16;

/// `handle_events` flushes the buffered batch to the write thread once it
/// grows past this size.
const EVENT_BATCH_SIZE: usize = 8192;

/// One axis of a declared histogram. `min`/`max` are the values represented
/// by the first and last bin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtAxisSpec {
    pub name: String,
    pub bins: u16,
    pub min: f64,
    pub max: f64,
}

/// Declares one histogram an external consumer publishes over its own shm
/// segment, named `<ipc_name>_<output_name>_<histo_name>` as metadata for
/// client discovery.  UMAMI itself does not touch this segment; the consumer
/// needs to do it, e.g. using the ShmWriter exported to Python. `y`/`t` are
/// optional, so a histogram can be 1-D, 2-D, or 3-D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtHistoSpec {
    pub name: String,
    pub x: ExtAxisSpec,
    #[serde(default)]
    pub y: Option<ExtAxisSpec>,
    #[serde(default)]
    pub t: Option<ExtAxisSpec>,
}

#[derive(Debug, Deserialize)]
struct ExtProcessConfig {
    #[serde(default)]
    histos: Vec<ExtHistoSpec>,
}

/// Forwards the full sorted event stream to a single external consumer, over
/// an abstract-namespace Unix stream socket named `<ipc_name>_<output_name>`,
/// for processing outside UMAMI. See `docs/outputs.md` for the wire format.
/// Only one connection is allowed, a second connection replaces the first.
#[derive(HasParams)]
#[params(kind = "output", type = "ext_process")]
pub struct ExtProcessOutput {
    name: ModuleId,
    #[param(help = "Histogram(s) the external consumer publishes, for client discovery",
            readonly = true, datatype = "array of histogram specs")]
    histos: Vec<ExtHistoSpec>,
    buffer: Vec<Event>,
    #[allow(dead_code, reason = "only read by tests, to observe the io thread's connection state")]
    connected: Arc<AtomicBool>,
    seq_number: u16,
    dropped_this_run: bool,
    frame_send: Sender<(u16, Frame)>,
}

impl Output for ExtProcessOutput {
    fn from_config(common: &OutputCommon, config: toml::Table) -> UResult<Self> {
        let config: ExtProcessConfig = config.try_into()
            .context("Parsing ext_process output config")?;

        let sock_name = format!("{}_{}", common.ipc_name, common.name);
        let addr = UnixSocketAddr::from_abstract(sock_name.as_bytes())
            .context("Creating abstract socket address")?;
        let listener = UnixListener::bind_unix_addr(&addr)
            .context("Binding ext_process event socket")?;
        listener.set_nonblocking(true)
                .context("Setting ext_process listener nonblocking")?;

        let (frame_send, frame_recv) = channel::bounded(FRAME_QUEUE_SIZE);
        let connected = Arc::new(AtomicBool::new(false));
        let io_connected = Arc::clone(&connected);
        let name = common.name;
        std::thread::Builder::new()
            .name(format!("O: {name} io"))
            .spawn(move || Self::run_io(listener, frame_recv, io_connected, name))
            .context("Spawning ext_process io thread")?;

        let histos = validate_histos(config.histos)?;
        Ok(Self { name, histos, connected, seq_number: 0, dropped_this_run: false,
                  buffer: Vec::with_capacity(2 * EVENT_BATCH_SIZE), frame_send })
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        self.buffer.extend(
            events.iter().filter(|e| e.evtype == EventType::Neutron).cloned());
        if self.buffer.len() > EVENT_BATCH_SIZE {
            let batch = replace(&mut self.buffer, Vec::with_capacity(2 * EVENT_BATCH_SIZE));
            let seq = self.next_seq();
            // droppable: this is a live view, not archival, so losing a
            // batch to keep the pipeline at full speed is the right trade
            // when the consumer can't keep up
            if self.frame_send.try_send((seq, Frame::Events(batch))).is_err()
                && !self.dropped_this_run {
                lprintln!(WARN, [self.name] "dropped an event batch, consumer too slow");
                self.dropped_this_run = true;
            }
        }
        Ok(())
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        self.buffer.clear();
        self.dropped_this_run = false;
        let seq = self.next_seq();
        let _ = self.frame_send.send((seq, Frame::StartOfRun(run.into())));
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        if !self.buffer.is_empty() {
            let batch = replace(&mut self.buffer, Vec::with_capacity(2 * EVENT_BATCH_SIZE));
            let seq = self.next_seq();
            let _ = self.frame_send.send((seq, Frame::Events(batch)));
        }
        let seq = self.next_seq();
        let _ = self.frame_send.send((seq, Frame::EndOfRun));
        Ok(())
    }

    fn handle_clear(&mut self) -> UResult<()> {
        self.buffer.clear();
        let seq = self.next_seq();
        let _ = self.frame_send.send((seq, Frame::Clear));
        Ok(())
    }
}

fn validate_histos(value: Vec<ExtHistoSpec>) -> UResult<Vec<ExtHistoSpec>> {
    let mut seen = HashSet::new();
    for h in &value {
        if !seen.insert(h.name.clone()) {
            return Err(anyhow!("Duplicate histogram name {:?}", h.name).into());
        }
        let zero_bins = h.x.bins == 0
            || h.y.as_ref().is_some_and(|a| a.bins == 0)
            || h.t.as_ref().is_some_and(|a| a.bins == 0);
        if zero_bins {
            return Err(anyhow!("Histogram {:?} has an axis with zero bins", h.name).into());
        }
    }
    Ok(value)
}

impl ExtProcessOutput {
    fn next_seq(&mut self) -> u16 {
        let seq = self.seq_number;
        self.seq_number = self.seq_number.wrapping_add(1);
        seq
    }

    /// Accepts connections and writes queued frames to whichever one is
    /// current.
    fn run_io(listener: UnixListener, frame_recv: Receiver<(u16, Frame)>,
              connected: Arc<AtomicBool>, name: ModuleId) {
        let mut conn: Option<UnixStream> = None;
        loop {
            if let Ok((stream, _)) = listener.accept() {
                stream.set_write_timeout(Some(Duration::from_millis(500)))
                      .expect("set_write_timeout on unix stream failed");
                lprintln!(INFO, [name] "accepted connection from {:?}", stream.peer_addr().ok());
                conn = Some(stream);
                connected.store(true, Ordering::Relaxed);
            }
            match frame_recv.recv_timeout(Duration::from_millis(20)) {
                Ok((seq, frame)) => {
                    let (tag, payload) = match &frame {
                        Frame::Events(p) => (FrameTag::Events, p.as_bytes()),
                        Frame::StartOfRun(p) => (FrameTag::StartOfRun, p.as_bytes()),
                        Frame::EndOfRun => (FrameTag::EndOfRun, &[][..]),
                        Frame::Clear => (FrameTag::Clear, &[][..]),
                    };
                    if let Some(stream) = &mut conn {
                        let seq = seq.to_le_bytes();
                        let len = (payload.len() as u32).to_le_bytes();
                        let ok = stream.write_all(&[tag.into(), seq[0], seq[1],
                                                    len[0], len[1], len[2], len[3]])
                            .and_then(|()| stream.write_all(payload));
                        if ok.is_err() {
                            lprintln!(WARN, [name] "lost connection");
                            conn = None; // dead connection; next accept() replaces it
                        }
                    }
                    connected.store(conn.is_some(), Ordering::Relaxed);
                }
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Duration;
    use uds::UnixStreamExt;
    use zerocopy::TryFromBytes;
    use crate::command::ModuleId;
    use crate::event::test_utils;
    use super::*;

    fn make_common(ipc_name: &str, out_name: &str) -> OutputCommon {
        let (_send, recv) = crate::channel::unbounded();
        OutputCommon::new(ModuleId::new(out_name.into()), ipc_name.into(), recv, None,
                          std::sync::Arc::new(crate::expr::AliasTable::new()))
    }

    fn connect(sock_name: &str) -> UnixStream {
        let addr = UnixSocketAddr::from_abstract(sock_name.as_bytes()).unwrap();
        // retry briefly: the accept thread starts asynchronously
        for _ in 0..100 {
            if let Ok(stream) = UnixStream::connect_to_unix_addr(&addr) {
                return stream;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("could not connect to {sock_name:?}");
    }

    /// `connect()` returning only means the kernel accepted the connection
    /// into its backlog -- the output's own io thread still needs to poll
    /// `accept()` and store the stream before `send_frame` will see it.
    fn wait_until_connected(output: &ExtProcessOutput) {
        for _ in 0..100 {
            if output.connected.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("output never registered the connection");
    }

    fn wait_until_disconnected(output: &ExtProcessOutput) {
        for _ in 0..100 {
            if !output.connected.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("output never noticed the disconnection");
    }

    fn recv_frame(stream: &mut UnixStream) -> (u16, FrameTag, Vec<u8>) {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut header = [0u8; 7];
        stream.read_exact(&mut header).unwrap();
        let tag = FrameTag::from(header[0]);
        let seq = u16::from_le_bytes(header[1..3].try_into().unwrap());
        let len = u32::from_le_bytes(header[3..7].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).unwrap();
        (seq, tag, payload)
    }

    #[test]
    fn test_no_consumer_does_not_block_or_error() {
        let common = make_common("umami_test_ext_process", "no_consumer");
        let mut output = ExtProcessOutput::from_config(&common, toml::Table::new()).unwrap();
        assert!(output.handle_events(&[test_utils::neutron(100, 5)]).is_ok());
        assert!(output.handle_start_of_run("run1").is_ok());
        assert!(output.handle_end_of_run().is_ok());
        assert!(output.handle_clear().is_ok());
    }

    #[test]
    fn test_forwards_events_start_end_clear() {
        let common = make_common("umami_test_ext_process", "forward");
        let mut output = ExtProcessOutput::from_config(&common, toml::Table::new()).unwrap();
        let mut stream = connect("umami_test_ext_process_forward");
        wait_until_connected(&output);

        output.handle_start_of_run("run1").unwrap();
        let (seq0, tag, payload) = recv_frame(&mut stream);
        assert_eq!(tag, FrameTag::StartOfRun);
        assert_eq!(payload, b"run1");

        // a single event stays buffered until enough accumulate for a
        // batch or the run ends; end_of_run flushes whatever is pending
        let event = test_utils::neutron(100, 5);
        output.handle_events(&[event]).unwrap();
        output.handle_end_of_run().unwrap();
        let (seq1, tag, payload) = recv_frame(&mut stream);
        assert_eq!(tag, FrameTag::Events);
        let batch = <[Event]>::try_ref_from_bytes(&payload).unwrap();
        assert_eq!(batch, [event]);
        let (seq2, tag, payload) = recv_frame(&mut stream);
        assert_eq!(tag, FrameTag::EndOfRun);
        assert!(payload.is_empty());
        assert_eq!([seq1, seq2], [seq0.wrapping_add(1), seq0.wrapping_add(2)]);

        output.handle_clear().unwrap();
        let (seq3, tag, payload) = recv_frame(&mut stream);
        assert_eq!(tag, FrameTag::Clear);
        assert!(payload.is_empty());
        assert_eq!(seq3, seq0.wrapping_add(3));
    }

    #[test]
    fn test_second_connection_replaces_first() {
        let common = make_common("umami_test_ext_process", "replace");
        let mut output = ExtProcessOutput::from_config(&common, toml::Table::new()).unwrap();
        let _first = connect("umami_test_ext_process_replace");
        wait_until_connected(&output);
        let mut second = connect("umami_test_ext_process_replace");
        // wait_until_connected() can't distinguish "first" from "second" via
        // the connected flag alone; give the io thread's poll loop one more
        // cycle to pick up the second connection and replace the first.
        std::thread::sleep(Duration::from_millis(100));

        output.handle_start_of_run("run1").unwrap();
        let (_, tag, payload) = recv_frame(&mut second);
        assert_eq!(tag, FrameTag::StartOfRun);
        assert_eq!(payload, b"run1");
    }

    #[test]
    fn test_declared_histos_are_reported_in_params() {
        let common = make_common("umami_test_ext_process", "histos1");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "physical_view"
            x = { name = "qx", bins = 200, min = -2.0, max = 2.0 }
            y = { name = "qy", bins = 100, min = -1.0, max = 1.0 }
        "#).unwrap();
        let output = ExtProcessOutput::from_config(&common, cfg).unwrap();

        let params = output.get_params(false).unwrap();
        assert_eq!(params["histos"]["value"][0]["name"], "physical_view");
        assert_eq!(params["histos"]["value"][0]["x"]["name"], "qx");
        assert_eq!(params["histos"]["value"][0]["x"]["bins"], 200);
        assert_eq!(params["histos"]["value"][0]["x"]["min"], -2.0);
        assert_eq!(params["histos"]["value"][0]["x"]["max"], 2.0);
        assert_eq!(params["histos"]["value"][0]["y"]["bins"], 100);
        assert!(params["histos"]["value"][0]["t"].is_null(), "t is optional");
    }

    #[test]
    fn test_histo_can_be_1d_2d_or_3d() {
        let common = make_common("umami_test_ext_process", "histos5");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "one_d"
            x = { name = "qx", bins = 10, min = 0.0, max = 9.0 }

            [[histos]]
            name = "three_d"
            x = { name = "qx", bins = 10, min = 0.0, max = 9.0 }
            y = { name = "qy", bins = 5, min = 0.0, max = 4.0 }
            t = { name = "time", bins = 3, min = 0.0, max = 2.0 }
        "#).unwrap();
        let output = ExtProcessOutput::from_config(&common, cfg).unwrap();
        assert!(output.histos[0].y.is_none() && output.histos[0].t.is_none());
        assert!(output.histos[1].y.is_some() && output.histos[1].t.is_some());
    }

    #[test]
    fn test_zero_bins_axis_rejected() {
        let common = make_common("umami_test_ext_process", "histos6");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "bad"
            x = { name = "qx", bins = 0, min = 0.0, max = 9.0 }
        "#).unwrap();
        assert!(ExtProcessOutput::from_config(&common, cfg).is_err());
    }

    /// Simulates what an external `ext_process` consumer does: create its
    /// own shm segment named per the documented convention and write to it,
    /// independently of UMAMI -- then verify a reader finds exactly what it
    /// wrote, proving the declared spec and the naming convention actually
    /// line up (not just that each half works in isolation).
    #[test]
    fn test_declared_histo_name_matches_convention_an_external_writer_would_use() {
        let ipc_name = "umami_test_ext_process_shm";
        let common = make_common(ipc_name, "histos4");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "physical_view"
            x = { name = "qx", bins = 4, min = 0.0, max = 3.0 }
            y = { name = "qy", bins = 3, min = 0.0, max = 2.0 }
        "#).unwrap();
        let output = ExtProcessOutput::from_config(&common, cfg).unwrap();
        let spec = &output.histos[0];
        let (nx, ny) = (spec.x.bins, spec.y.as_ref().unwrap().bins);

        let shm_name = format!("{ipc_name}_histos4_{}", spec.name);
        let _guard = crate::shm::ShmGuard::for_name(shm_name.clone());
        let histo_config = crate::config::HistoConfig {
            nx: nx as usize, ny: ny as usize, max_nt: 1, max_ni: 0,
        };
        let mut writer = crate::shm::ShmWriter::create(&shm_name, &histo_config).unwrap();
        writer.set_run_id("sim_run");
        writer.add_histo(crate::event::EventHisto { x: 2, y: 1, t: 0, i: 0 });

        let reader = crate::shm::ShmReader::open(&shm_name).unwrap();
        assert_eq!(reader.run_id(), "sim_run");
        assert_eq!((reader.nx(), reader.ny()), (nx, ny));
        assert_eq!(reader.histo_data()[nx as usize + 2], 1); // y=1, x=2, nx=4
    }

    #[test]
    fn test_duplicate_histo_names_rejected_at_config_time() {
        let common = make_common("umami_test_ext_process", "histos2");
        let cfg: toml::Table = toml::from_str(r#"
            [[histos]]
            name = "dup"
            x = { name = "qx", bins = 10, min = 0.0, max = 9.0 }

            [[histos]]
            name = "dup"
            x = { name = "qx", bins = 20, min = 0.0, max = 19.0 }
        "#).unwrap();
        assert!(ExtProcessOutput::from_config(&common, cfg).is_err());
    }

    #[test]
    fn test_dead_connection_is_dropped_not_fatal() {
        let common = make_common("umami_test_ext_process", "dead");
        let mut output = ExtProcessOutput::from_config(&common, toml::Table::new()).unwrap();
        let stream = connect("umami_test_ext_process_dead");
        wait_until_connected(&output);
        drop(stream);
        // give the io thread a moment to observe the close
        std::thread::sleep(Duration::from_millis(50));
        // a write to a closed peer may need one attempt to be observed
        output.handle_start_of_run("run1").unwrap();
        output.handle_start_of_run("run2").unwrap();
        wait_until_disconnected(&output);
    }
}
