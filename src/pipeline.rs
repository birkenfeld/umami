// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::Path;
use anyhow::{anyhow, Context};
use crate::output::OutputCommon;
use crate::{channel, ldebug, lprintln, output};
use crate::{command, input, postproc, recipe, sorter};
use crate::command::{Command, CommandReply};
use crate::config::{Config, OutputConfig};
use crate::error::UResult;
use crate::event::{Event, ModuleId};
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
    SetMode(String, channel::Sender<CommandReply>),
    GetParams(channel::Sender<(String, ParamMap)>),
    SetParams(BTreeMap<String, ParamMap>, channel::Sender<CommandReply>),
    GetState(channel::Sender<CommandReply>),
    SaveHisto(String, usize, channel::Sender<CommandReply>),
}

pub fn run_pipeline(config: Config, immediate_start: bool) -> UResult<()> {
    let n_inputs = config.inputs.len();
    if n_inputs == 0 {
        Err(anyhow!("No inputs configured"))?;
    } else if n_inputs > MAX_INPUTS {
        Err(anyhow!("Too many inputs: {}, max is {}", n_inputs, MAX_INPUTS))?;
    }

    let shm_area = ShmInterface::create(&config.ipc_name, &config.histogram)?;

    lprintln!(INFO, "IPC interfaces are available under the name {:?}", config.ipc_name);

    let (postproc_send, postproc_recv) = channel::bounded(EV_CHANNEL_SIZE);

    let mut init_errors = false;
    let mut pipe_recvs = vec![];
    let mut command_sends = BTreeMap::new();
    let confdir = config.filename.parent().unwrap_or_else(|| Path::new("."));

    for (input_name, input_config) in config.inputs {
        let mid = ModuleId(input_config.id);
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
            mid, postproc_send.clone(), event_send, command_recv,
            recipe::from_config(&config.input_recipes, &input_config.recipe)?
        );
        command_sends.insert(mid, command_send);

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

    for (name, out_config) in outputs {
        ldebug!("Initializing output {name}: {:?}", out_config);
        let common = OutputCommon::new(name.clone(),
                                       output_recvs.pop().expect("one per output"),
                                       output_sends.pop());
        if let Err(e) = output::start(out_config, common) {
            init_errors = true;
            lprintln!(ERROR, "Failed to initialize output {}: {e:#}", name);
        }
    }
    if init_errors {
        Err(anyhow!("Some outputs failed to initialize"))?;
    }

    let mut post_recipes = BTreeMap::new();
    for name in config.process_modes.recipes.keys() {
        post_recipes.insert(name.into(), recipe::from_config(&config.process_modes.recipes, &name)?);
    }
    if !post_recipes.contains_key(&config.process_modes.default) {
        Err(anyhow!("No default mode configured"))?;
    }

    let postproc = postproc::PostProcessor::new(
        post_recipes,
        config.process_modes.default,
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

    lprintln!(INFO, "Init done, waiting for commands");
    handler.start()?;

    wait_for_signal().context("Setting signal handler")?;

    Ok(())
}
