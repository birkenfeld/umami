// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use crate::{channel, lprintln, ldebug};
use crate::{command, input, postproc, recipe, sorter};
use crate::config::Config;
use crate::error::UResult;
use crate::event::{Event, ModuleId};
use crate::input::{InputCommon, InputState};
use crate::interface::UdsInterface;
use crate::shm::{ShmInterface, MAX_MODULES};
use crate::util::wait_for_signal;

const EV_CHANNEL_SIZE: usize = 64; // TODO tune

pub enum PipeItem {
    Events(Vec<Event>),
    Clear,
    StartOfRun(String),
    EndOfRun,
    State(InputState),
    TofParams { nt: usize, dt: f64, t0: f64 },
}

pub fn run_pipeline(config: Config, immediate_start: bool) -> UResult<()> {
    let n_modules = config.modules.len();
    if n_modules == 0 {
        Err(anyhow!("No modules configured"))?;
    } else if n_modules > MAX_MODULES {
        Err(anyhow!("Too many modules: {}, max is {}", n_modules, MAX_MODULES))?;
    }

    let mut shm_area = ShmInterface::create(&config.ipc_name, &config.histogram)?;
    shm_area.reset(n_modules as u32);

    let (if_cmd_send, if_cmd_recv) = channel::bounded(1);  // only one command at a time
    let (if_reply_send, if_reply_recv) = channel::bounded(1);
    let uds = UdsInterface::new(&config.ipc_name, if_cmd_send, if_reply_recv)?;

    lprintln!(INFO, "IPC interfaces are available under the name {:?}", config.ipc_name);

    let (cmd_reply_send, cmd_reply_recv) = channel::bounded(n_modules + 1);

    let (postproc_send, postproc_recv) = channel::bounded(EV_CHANNEL_SIZE);

    let mut init_errors = false;
    let mut pipe_recvs = vec![];
    let mut command_sends = BTreeMap::new();

    for (module_name, module_config) in config.modules {
        let mid = ModuleId(module_config.id);
        ldebug!("Initializing module {}: {:?}", module_name, module_config);

        let event_send = if n_modules == 1 {
            postproc_send.clone()
        } else {
            let (send, recv) = channel::bounded(EV_CHANNEL_SIZE);
            pipe_recvs.push(recv);
            send
        };
        let (command_send, command_recv) = channel::bounded(1);
        let common = InputCommon {
            needs_reset: false,
            running: immediate_start,
            module: mid,
            events: event_send,
            state: postproc_send.clone(),
            command: command_recv,
            command_reply: cmd_reply_send.clone(),
            recipe: recipe::from_config(&config.recipes, &module_config.recipe)?,
        };
        command_sends.insert(mid, command_send);

        if let Err(e) = input::start(module_config.specific, common) {
            lprintln!(ERROR, "Failed to initialize module {}: {}", module_name, e);
            init_errors = true;
        }
    }
    drop(cmd_reply_send);  // leftover sender
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
    command::CommandHandler::start(
        if_cmd_recv,
        command_sends,
        cmd_reply_recv,
        if_reply_send,
        postproc_send.clone(),
    )?;

    let postproc = postproc::PostProcessor::new(
        recipe::from_config(&config.recipes, &config.postprocess.recipe)?,
        postproc_recv,
        shm_area,
    );

    uds.start()?;
    postproc.start()?;

    wait_for_signal().context("Setting signal handler")?;

    Ok(())
}
