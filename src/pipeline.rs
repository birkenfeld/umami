// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use crate::output::OutputCommon;
use crate::{channel, ldebug, lprintln, output};
use crate::{command, input, postproc, recipe, sorter};
use crate::command::CommandReply;
use crate::config::{Config, OutputConfig};
use crate::error::UResult;
use crate::event::{Event, ModuleId};
use crate::input::{ModuleCommon, ModuleState};
use crate::shm::{ShmInterface, MAX_MODULES};
use crate::util::wait_for_signal;

const EV_CHANNEL_SIZE: usize = 128; // TODO tune more

pub enum PipeItem {
    Events(Vec<Event>),
    Clear,
    StartOfRun(String),
    EndOfRun,
    ModuleState(ModuleId, ModuleState),
    SetMode(String, toml::Table, channel::Sender<CommandReply>),
    GetState(channel::Sender<CommandReply>),
}

pub fn run_pipeline(config: Config, immediate_start: bool) -> UResult<()> {
    let n_modules = config.modules.len();
    if n_modules == 0 {
        Err(anyhow!("No modules configured"))?;
    } else if n_modules > MAX_MODULES {
        Err(anyhow!("Too many modules: {}, max is {}", n_modules, MAX_MODULES))?;
    }

    let shm_area = ShmInterface::create(&config.ipc_name, &config.histogram)?;

    lprintln!(INFO, "IPC interfaces are available under the name {:?}", config.ipc_name);

    let (postproc_send, postproc_recv) = channel::bounded(EV_CHANNEL_SIZE);

    let mut init_errors = false;
    let mut pipe_recvs = vec![];
    let mut command_sends = BTreeMap::new();

    for (module_name, module_config) in config.modules {
        let mid = ModuleId(module_config.id);
        ldebug!("Initializing module {module_name}: {:?}", module_config);

        let event_send = if n_modules == 1 {
            postproc_send.clone()
        } else {
            let (send, recv) = channel::bounded(EV_CHANNEL_SIZE);
            pipe_recvs.push(recv);
            send
        };
        let (command_send, command_recv) = channel::bounded(1);
        let common = ModuleCommon::new(
            mid, postproc_send.clone(), event_send, command_recv,
            recipe::from_config(&config.recipes, &module_config.recipe)?
        );
        command_sends.insert(mid, command_send);

        if let Err(e) = input::start(module_config.specific, common) {
            lprintln!(ERROR, "Failed to initialize module {module_name}: {e:#}");
            init_errors = true;
        }
    }
    if init_errors {
        Err(anyhow!("Some modules failed to initialize"))?;
    }

    // create event sorters if we have more than one module
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
        postproc_send.clone(),
    ).context("Creating command handler")?;

    let outputs = match config.outputs {
        Some(outputs) => outputs,
        None => {
            lprintln!(INFO, "No outputs configured, using null output as fallback.");
            Vec::from([OutputConfig {r#type: "none".to_string(), config: toml::Table::default()}])
            // BTreeMap::from([(
            //     "null".to_string(),
            //     OutputConfig {r#type: "none".to_string(), config: toml::Table::default()},
            // )])
        },
    };
    let (output_send, output_recv) = channel::bounded(EV_CHANNEL_SIZE);
    let mut next_output = output_recv;

    let (last_output, other_outputs) = outputs.split_last().expect("Inserted default");

    for out_config in other_outputs.into_iter() {
        let (output_send, output_recv) = channel::bounded(EV_CHANNEL_SIZE);
        let common = OutputCommon::new(next_output, Some(output_send));
        next_output = output_recv;
        if let Err(e) = output::from_config(out_config.clone())?.start(common) {
            let t = &out_config.r#type;
            init_errors = true;
            lprintln!(ERROR, "Failed to initialize output {t}: {e:#}");
        }
    }
    {
        let common = OutputCommon::new(next_output, None);
        if let Err(e) = output::from_config(last_output.clone())?.start(common) {
            let t = &last_output.r#type;
            init_errors = true;
            lprintln!(ERROR, "Failed to initialize output {t}: {e:#}");
        }
    }
    if init_errors {
        Err(anyhow!("Some outputs failed to initialize"))?;
    }

    let mut post_recipes = BTreeMap::new();
    for (name, recipe) in config.process_modes {
        post_recipes.insert(name, recipe::from_config(&config.recipes, &recipe)?);
    }
    if !post_recipes.contains_key("default") {
        Err(anyhow!("No default mode configured"))?;
    }

    let postproc = postproc::PostProcessor::new(
        post_recipes,
        postproc_recv,
        output_send,
        shm_area,
    );


    postproc.start()?;

    if immediate_start {
        handler.handle(command::Command::Start { run_id: "auto".to_string() });
    }
    handler.start()?;

    wait_for_signal().context("Setting signal handler")?;

    Ok(())
}
