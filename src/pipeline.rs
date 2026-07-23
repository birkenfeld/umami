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
use crate::util::wait_for_signal;

const EV_CHANNEL_SIZE: usize = 128; // TODO tune more?
const OUT_CHANNEL_SIZE: usize = 16384; // give outputs some slack

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

pub fn run_pipeline(config: Config, immediate_start: bool) -> UResult<()> {
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

    // create event sorters if we have more than one input
    while pipe_recvs.len() > 1 {
        let read_1 = pipe_recvs.pop().expect("len checked");
        let read_2 = pipe_recvs.pop().expect("len checked");
        if pipe_recvs.is_empty() {
            // this is the last sorter, write directly to the final channel
            sorter::Sorter::new(read_1, read_2, postproc_send.clone()).start()?;
        } else {
            let (write, sorted_recv) = channel::bounded(EV_CHANNEL_SIZE);
            sorter::Sorter::new(read_1, read_2, write).start()?;
            pipe_recvs.push(sorted_recv);
        }
    }

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

    wait_for_signal().context("Setting signal handler")?;

    Ok(())
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
    use crate::output::Output;
    use crate::params::HasParams;

    static SHM_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestOutput {
        done: channel::Sender<()>,
    }

    impl HasParams for TestOutput {
        fn get_params(&self) -> UResult<ParamMap> { Ok(ParamMap::new()) }
        fn update_params(&mut self, _: ModuleId, _: ParamMap) -> UResult<()> { Ok(()) }
    }

    impl Output for TestOutput {
        fn from_config(_: &OutputCommon, _: toml::Table) -> UResult<Self> {
            unreachable!()
        }
        fn handle_events(&mut self, _: &[Event]) -> UResult<()> { Ok(()) }
        fn handle_start_of_run(&mut self, _: &str) -> UResult<()> { Ok(()) }
        fn handle_end_of_run(&mut self) -> UResult<()> {
            self.done.send(()).ok();
            Ok(())
        }
    }

    struct TestPipeline {
        done_rx: channel::Receiver<()>,
        shm_name: String,
        ipc_name: String,
    }

    impl TestPipeline {
        fn new(config: Config) -> Self {
            let shm_name = format!(
                "umami_inttest_{}_{}", std::process::id(),
                SHM_COUNTER.fetch_add(1, Ordering::SeqCst),
            );
            let ipc_name = format!(
                "umami_test_{}_{}", std::process::id(),
                SHM_COUNTER.fetch_add(1, Ordering::SeqCst),
            );
            let confdir = config.filename.parent().unwrap_or(Path::new("."));

            let shm = ShmInterface::create(&shm_name, &config.histogram)
                .expect("Could not create shared memory for test");

            let mut post_recipes = BTreeMap::new();
            for name in config.process_modes.recipes.keys() {
                let recipe_name = ModuleId::new(name.into());
                post_recipes.insert(
                    recipe_name,
                    recipe::from_config(&config.process_modes.recipes, &recipe_name)
                        .expect("Could not initialize postprocessor recipe"),
                );
            }
            let default_name = ModuleId::new(config.process_modes.default);

            let (postproc_send, postproc_recv) = channel::bounded(EV_CHANNEL_SIZE);
            let (output_send, output_recver) = channel::bounded(OUT_CHANNEL_SIZE);

            postproc::PostProcessor::new(
                post_recipes, default_name, postproc_recv, output_send, shm,
            ).start().expect("Could not start postprocessor");

            let (done_tx, done_rx) = channel::bounded(1);
            let out_common = OutputCommon::new(
                ModuleId::new("test_out".into()), output_recver, None,
            );
            TestOutput { done: done_tx }.start(out_common)
                .expect("Could not start test output");

            let mut command_sends = BTreeMap::new();
            for (input_name, input_config) in config.inputs {
                let input_recipe = recipe::from_config(
                    &config.input_recipes, &input_config.recipe,
                ).expect("Initializing input recipe");
                let (cmd_tx, cmd_rx) = channel::bounded(1);
                let common = InputCommon::new(
                    ModuleId::new(input_name.clone()),
                    postproc_send.clone(),
                    postproc_send.clone(),
                    cmd_rx,
                    input_recipe,
                );
                input::start(input_config.specific, confdir, common)
                    .expect("Could not start input");
                command_sends.insert(ModuleId::new(input_name), cmd_tx);
            }

            let handler = command::CommandHandler::new(
                &ipc_name, command_sends, postproc_send,
            ).expect("Could not create command handler");
            handler.start().expect("Could not start command handler");

            Self { done_rx, shm_name, ipc_name }
        }

        fn send_command(&self, cmd: &Command) -> CommandReply {
            let client = Client::new(&self.ipc_name)
                .expect("Creating test client");
            let reply = client.send(cmd).expect("Command got no reply");
            if reply.is_error() {
                panic!("Command {:?} failed: {:?}", cmd, reply);
            } else {
                reply
            }
        }

        fn wait_for_completion(&self) {
            self.done_rx.recv_timeout(std::time::Duration::from_secs(30))
                  .expect("Pipeline did not complete in time");
        }
    }

    #[test]
    fn test_pipeline_mesy_file() {
        let conf_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/mesyfile.conf");
        let config = crate::load_config(&conf_path).expect("Loading test config");

        let pipeline = TestPipeline::new(config);
        pipeline.send_command(&Command::Start { run_id: "test".into() });
        pipeline.wait_for_completion();

        let shm_read = ShmInterface::open(&pipeline.shm_name).unwrap();
        let total = shm_read.histo_total();
        assert!(total > 0, "Expected non-zero neutron counts in histogram");

        nix::sys::mman::shm_unlink(pipeline.shm_name.as_bytes()).ok();
    }
}
