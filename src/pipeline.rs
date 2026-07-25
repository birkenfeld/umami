// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::Path;
use anyhow::{anyhow, Context};
use crate::output::OutputCommon;
use crate::{channel, ldebug, lprintln, output};
use crate::{command, input, postproc, recipe, sorter};
use crate::command::{Command, CommandReply, ModuleId};
use crate::config::{Config, OutputConfig};
use crate::error::UResult;
use crate::event::Event;
use crate::input::{InputCommon, InputState};
use crate::params::ParamMap;
use crate::shm::{ShmInterface, MAX_INPUTS};

// Determined using profile_event_channel_occupancy.
pub(crate) const EV_CHANNEL_SIZE: usize = 2048;
const OUT_CHANNEL_SIZE: usize = 16384; // give outputs some slack

// Tracks the highest occupancy seen on any input->sorter/postprocessor
// events channel; see profile_event_channel_occupancy below.
#[cfg(feature = "profile")]
pub(crate) static MAX_EVENTS_CHANNEL_LEN: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub struct PipelineHandle {
    ipc_name: String,
}

impl PipelineHandle {
    pub fn ipc_name(&self) -> &str {
        &self.ipc_name
    }

    pub fn shm_name(&self) -> &str {
        &self.ipc_name
    }
}

#[derive(Debug)]
pub enum PipeItem {
    Events(Vec<Event>),
    Clear,
    StartOfRun(String),
    EndOfRun,
    InputState(ModuleId, InputState),
    GetModes(channel::Sender<CommandReply>),
    SetMode(ModuleId, channel::Sender<CommandReply>),
    GetParams(channel::Sender<(ModuleId, ParamMap)>),
    SetParams(BTreeMap<ModuleId, ParamMap>, channel::Sender<CommandReply>),
    GetState(channel::Sender<CommandReply>),
    SaveHisto(String, usize, channel::Sender<CommandReply>),
}

pub fn start_pipeline(config: Config, immediate_start: bool) -> UResult<PipelineHandle> {
    let n_inputs = config.inputs.len();
    if n_inputs == 0 {
        Err(anyhow!("No inputs configured"))?;
    } else if n_inputs > MAX_INPUTS {
        Err(anyhow!("Too many inputs: {n_inputs}, max is {MAX_INPUTS}"))?;
    }

    let (postproc_send, postproc_recv) = channel::bounded(EV_CHANNEL_SIZE);

    // check uniqueness of names for modules (input, output, recipes)
    check_module_names(&config)?;

    // create inputs
    let mut init_errors = false;
    let mut pipe_recvs = vec![];
    let mut command_sends = BTreeMap::new();
    let confdir = config.filename.parent().unwrap_or_else(|| Path::new("."));

    for (input_name, input_config) in config.inputs {
        let input_name = ModuleId::new(input_name);
        ldebug!("Initializing input {input_name}: {:?}", input_config);

        let event_send = if n_inputs == 1 {
            postproc_send.clone()
        } else {
            let (send, recv) = channel::bounded(EV_CHANNEL_SIZE);
            pipe_recvs.push(recv);
            send
        };
        let (command_send, command_recv) = channel::bounded(1);
        let common = InputCommon::new(
            input_name, postproc_send.clone(), event_send, command_recv,
            recipe::from_config(&config.input_recipes, &input_config.recipe)?
        );
        command_sends.insert(input_name, command_send);

        if let Err(e) = input::start(input_config.specific, confdir, common) {
            lprintln!(ERROR, "Failed to initialize input {input_name}: {e:#}");
            init_errors = true;
        }
    }
    if init_errors {
        Err(anyhow!("Some inputs failed to initialize"))?;
    }

    // merge all inputs into one stream; no-op for < 2 inputs
    sorter::build_sorter_tree(pipe_recvs, postproc_send.clone())?;

    // create command handler
    let handler = command::CommandHandler::new(
        &config.ipc_name,
        command_sends,
        postproc_send,
    ).context("Creating command handler")?;

    // handle outputs - we need to have at least a null output to consume from the postproc
    let mut outputs = config.outputs.unwrap_or_default();
    if outputs.is_empty() {
        outputs.insert("null".into(), OutputConfig { r#type: "none".to_string(),
                                                     config: Default::default() });
    }

    // create channels between outputs (they are daisy-chained)
    let (mut output_sends, mut output_recvs): (Vec<_>, Vec<_>) =
        (0..outputs.len()).map(|_| channel::bounded(OUT_CHANNEL_SIZE)).unzip();
    let first_output_send = output_sends.pop().expect("at least one");

    for (out_name, out_config) in outputs {
        let out_name = ModuleId::new(out_name);
        ldebug!("Initializing output {out_name}: {:?}", out_config);
        let common = OutputCommon::new(out_name,
                                       output_recvs.pop().expect("one per output"),
                                       output_sends.pop());
        if let Err(e) = output::start(out_config, common) {
            init_errors = true;
            lprintln!(ERROR, "Failed to initialize output {out_name}: {e:#}");
        }
    }
    if init_errors {
        Err(anyhow!("Some outputs failed to initialize"))?;
    }

    // create the postprocessor
    let mut post_recipes = BTreeMap::new();
    for name in config.process_modes.recipes.keys() {
        let recipe_name = ModuleId::new(name.into());
        post_recipes.insert(
            recipe_name,
            recipe::from_config(&config.process_modes.recipes, &recipe_name)?,
        );
    }
    let default_name = ModuleId::new(config.process_modes.default);
    if !post_recipes.contains_key(&default_name) {
        Err(anyhow!("No default mode configured"))?;
    }

    let shm_area = ShmInterface::create(&config.ipc_name, &config.histogram)?;
    let postproc = postproc::PostProcessor::new(
        post_recipes,
        default_name,
        postproc_recv,
        first_output_send,
        shm_area,
        config.name.clone(),
    );
    postproc.start()?;

    if let Some(path) = config.raw_dir {
        lprintln!(INFO, "Raw dump enabled, dumping to {:?}", path.display());
        let path = path.to_string_lossy().to_string();
        handler.handle(Command::SetRawDump { enable: true, path });
    }
    if immediate_start {
        handler.handle(Command::Start { run_id: "auto".to_string() });
    }

    lprintln!(INFO, "Init done, IPC interfaces are available under the name {:?}",
              config.ipc_name);

    handler.start()?;

    Ok(PipelineHandle { ipc_name: config.ipc_name })
}

fn check_module_names(config: &Config) -> UResult<()> {
    let mut all_names = BTreeMap::new();
    for input_name in config.inputs.keys() {
        if let Some(prev) = all_names.insert(input_name, "input") {
            Err(anyhow!("Duplicate module name: input {input_name}, \
                         already used as {prev}"))?;
        }
    }
    for output_name in config.outputs.as_ref().map(|k| k.keys()).into_iter().flatten() {
        if let Some(prev) = all_names.insert(output_name, "output") {
            Err(anyhow!("Duplicate module name: output {output_name}, \
                         already used as {prev}"))?;
        }
    }
    for recipe_name in config.process_modes.recipes.keys() {
        if let Some(prev) = all_names.insert(recipe_name, "process mode") {
            Err(anyhow!("Duplicate module name: mode {recipe_name}, \
                         already used as {prev}"))?;
        }
    }
    for recipe_name in config.input_recipes.keys() {
        if let Some(prev) = all_names.insert(recipe_name, "input recipe") {
            Err(anyhow!("Duplicate module name: input recipe {recipe_name}, \
                         already used as {prev}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::client::Client;
    use crate::config::{SourceConfig, SpecificInputConfig};

    static SHM_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Base URL of the file server hosting test data that is too large to
    /// keep in the repository (see .gitignore for test/data).
    const TEST_DATA_URL: &str = "https://forge.frm2.tum.de/public/umami-test";

    /// Downloads the file at `confdir`/`rel_path` from the test data server if
    /// it is not already present locally.
    fn ensure_test_data(confdir: &Path, rel_path: &str) {
        let dest = confdir.join(rel_path);
        if dest.exists() {
            return;
        }
        // local paths are conventionally "data/<subpath>"; the server mirrors
        // just "<subpath>" at its root
        let remote_rel = rel_path.strip_prefix("data/").unwrap_or(rel_path);
        let url = format!("{TEST_DATA_URL}/{remote_rel}");
        // cargo test captures the usual stdout/stderr writers (eprintln! et al.)
        let _ = nix::unistd::write(
            std::io::stderr(),
            format!("*** Downloading missing test data file {url:?} to {dest:?}\n").as_bytes(),
        );

        std::fs::create_dir_all(dest.parent().expect("dest has parent"))
            .expect("Creating test data directory");
        let mut response = ureq::get(&url).call()
            .unwrap_or_else(|e| panic!("Downloading test data {url}: {e}"));
        let mut file = std::fs::File::create(&dest)
            .unwrap_or_else(|e| panic!("Creating test data file {dest:?}: {e}"));
        std::io::copy(&mut response.body_mut().as_reader(), &mut file)
            .unwrap_or_else(|e| panic!("Writing test data file {dest:?}: {e}"));
    }

    /// Extracts the file source path of an input, if it uses a file (not IP) source.
    fn file_source_path(specific: &SpecificInputConfig) -> Option<&str> {
        let source = match specific {
            SpecificInputConfig::GE(cfg) => &cfg.source,
            SpecificInputConfig::Canon(cfg) => &cfg.source,
            SpecificInputConfig::Mesy(cfg) => &cfg.local,
            SpecificInputConfig::Test(_) => return None,
        };
        match source {
            SourceConfig::File(path) => Some(path),
            SourceConfig::IP(_) => None,
        }
    }

    /// Path of the golden (reference) histogram for `conf_name`, e.g.
    /// "test/mesy.conf" -> "test/mesy.golden.gz".
    fn golden_path(conf_name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(conf_name).with_extension("golden.gz")
    }

    /// Compares `histo` against the golden histogram for `conf_name`, gzip-compressed
    /// raw little-endian `u32` counts in pipeline order. If the environment variable
    /// `UMAMI_UPDATE_GOLDEN` is set, (re)writes the golden file from `histo` instead
    /// of comparing (use this deliberately after an intentional pipeline change).
    fn check_or_update_golden(conf_name: &str, histo: &[u32]) {
        use std::io::{Read, Write};
        use flate2::{Compression, read::GzDecoder, write::GzEncoder};

        let path = golden_path(conf_name);
        let bytes: Vec<u8> = histo.iter().flat_map(|v| v.to_le_bytes()).collect();

        if std::env::var_os("UMAMI_UPDATE_GOLDEN").is_some() {
            let file = std::fs::File::create(&path)
                .unwrap_or_else(|e| panic!("Creating golden histogram {path:?}: {e}"));
            let mut enc = GzEncoder::new(file, Compression::best());
            enc.write_all(&bytes).expect("Writing golden histogram");
            enc.finish().expect("Finishing golden histogram file");
            let _ = nix::unistd::write(
                std::io::stderr(),
                format!("*** Updated golden histogram {path:?}\n").as_bytes(),
            );
            return;
        }

        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!(
            "Opening golden histogram {path:?}: {e} (run with UMAMI_UPDATE_GOLDEN=1 to create it)"
        ));
        let mut golden_bytes = Vec::new();
        GzDecoder::new(file).read_to_end(&mut golden_bytes)
            .expect("Decompressing golden histogram");
        let golden: Vec<u32> = golden_bytes.chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().expect("chunk of 4")))
            .collect();

        assert_eq!(golden.len(), histo.len(),
                   "Histogram size mismatch vs golden {path:?}");
        if golden != histo {
            let ndiff = golden.iter().zip(histo).filter(|(g, h)| g != h).count();
            let sample: Vec<_> = golden.iter().zip(histo).enumerate()
                .filter(|(_, (g, h))| g != h)
                .take(10)
                .map(|(i, (g, h))| format!("[{i}]: golden={g} actual={h}"))
                .collect();
            panic!("Histogram mismatch vs golden {path:?}: {ndiff} of {} cells differ\n{}",
                   golden.len(), sample.join("\n"));
        }
    }

    /// Runs the pipeline defined by `conf_name` (relative to the crate root) to
    /// completion against a single run and returns the resulting histogram.
    fn run_pipeline_and_get_histo(conf_name: &str) -> Vec<u32> {
        let conf_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(conf_name);
        let mut config = crate::load_config(&conf_path)
            .expect("Loading test config");

        let confdir = conf_path.parent().expect("conf_path has parent");
        for input in config.inputs.values() {
            if let Some(path) = file_source_path(&input.specific) {
                ensure_test_data(confdir, path);
            }
        }

        let test_id = SHM_COUNTER.fetch_add(1, Ordering::SeqCst);
        config.ipc_name = format!(
            "umami_test_{}_{}", std::process::id(), test_id,
        );
        let run_id = format!("test_{test_id}");
        config.outputs.get_or_insert_with(BTreeMap::new).insert(
            "test".into(),
            OutputConfig { r#type: "test".into(), config: Default::default() },
        );

        let done_rx = crate::output::test::init_test_output(&run_id);
        let handle = start_pipeline(config, false)
            .expect("Starting test pipeline");

        let client = Client::new(handle.ipc_name())
            .expect("Creating test client");
        let reply = client.send(&Command::Start { run_id })
            .expect("Sending start command");
        assert!(matches!(reply, CommandReply::Ok), "Start command failed: {reply:?}");

        done_rx.recv_timeout(std::time::Duration::from_secs(30))
            .expect("Pipeline did not complete in time");

        let shm = crate::shm::ShmGuard::for_name(handle.shm_name());
        let shm_read = ShmInterface::open(shm.name())
            .expect("Opening shared memory for verification");
        shm_read.histo_data()
    }

    /// Runs the pipeline defined by `conf_name` and checks the resulting histogram
    /// against its golden reference.
    fn run_test_pipeline(conf_name: &str) {
        let histo = run_pipeline_and_get_histo(conf_name);
        check_or_update_golden(conf_name, &histo);
    }

    #[test]
    fn test_pipeline_mesy_file() {
        run_test_pipeline("test/mesy.conf");
    }

    #[test]
    fn test_pipeline_canon_file() {
        run_test_pipeline("test/canon.conf");
    }

    #[test]
    fn test_pipeline_ge_file() {
        run_test_pipeline("test/ge.conf");
    }

    /// Uses the synthetic "test" input backend (one Neutron event per histogram
    /// cell) to check the pipeline mechanics.
    #[test]
    fn test_pipeline_synthetic_input() {
        let histo = run_pipeline_and_get_histo("test/synthetic.conf");
        assert_eq!(histo.len(), 8 * 16);
        assert!(histo.iter().all(|&count| count == 1),
                "Expected exactly one count in every histogram cell, got {histo:?}");
    }

    /// Not a real test: replays each real dataset sequentially and reports the
    /// highest events-channel occupancy seen against EV_CHANNEL_SIZE, to inform
    /// whether that constant needs tuning. Run with:
    /// `cargo test --release --features profile -- profile --ignored --nocapture`.
    #[cfg(feature = "profile")]
    #[test]
    #[ignore]
    fn profile_event_channel_occupancy() {
        use std::sync::atomic::Ordering;
        for conf in ["test/mesy.conf", "test/canon.conf", "test/ge.conf"] {
            MAX_EVENTS_CHANNEL_LEN.store(0, Ordering::Relaxed);
            run_pipeline_and_get_histo(conf);
            let max = MAX_EVENTS_CHANNEL_LEN.load(Ordering::Relaxed);
            eprintln!("{conf}: max channel occupancy {max}/{EV_CHANNEL_SIZE}");
        }
    }
}
