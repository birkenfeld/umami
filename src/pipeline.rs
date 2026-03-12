// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::anyhow;
use crate::{channel, lprintln, ldebug};
use crate::{command, input, postproc, recipe, sorter};
use crate::config::Config;
use crate::error::UResult;
use crate::event::{Event, ModuleId};
use crate::input::{InputCommon, InputState};
use crate::interface::{UdsInterface, ShmInterface, MAX_MODULES};

const EV_CHANNEL_SIZE: usize = 64; // TODO tune

pub enum PipeItem {
    Events(Vec<Event>),
    Clear,
    StartOfRun(String),
    EndOfRun,
    State(InputState),
    TofParams { nt: usize, dt: f64, t0: f64 },
}


pub fn set_debug_params(debug: bool, trace: bool) -> UResult<()> {
    crate::DEBUG.store(debug, std::sync::atomic::Ordering::Relaxed);
    if cfg!(feature = "trace") {
        crate::TRACE.store(trace, std::sync::atomic::Ordering::Relaxed);
    } else if trace {
        Err(anyhow!("Trace logging is not available in this build"))?;
    }
    Ok(())
}

pub fn run_pipeline(config: Config, immediate_start: bool) -> UResult<()> {
    let n_modules = config.modules.len();
    if n_modules == 0 {
        Err(anyhow!("No modules configured"))?;
    } else if n_modules > MAX_MODULES {
        Err(anyhow!("Too many modules: {}, max is {}", n_modules, MAX_MODULES))?;
    }

    let mut shm = ShmInterface::map(&config.ipc_name)?;
    shm.reset(n_modules as u32);

    let (if_cmd_write, if_cmd_read) = channel::bounded(1);  // only one command at a time
    let (if_reply_write, if_reply_read) = channel::bounded(1);
    let uds = UdsInterface::new(&config.ipc_name, if_cmd_write, if_reply_read)?;

    lprintln!(INFO, "IPC interfaces are available under the name {:?}", config.ipc_name);

    let (state_write, state_read) = channel::bounded(n_modules + 1);
    let (cmd_reply_write, cmd_reply_read) = channel::bounded(n_modules + 1);

    let mut init_errors = false;
    let mut event_read_chans = vec![];
    let mut command_write_chans = BTreeMap::new();
    let mut last_pipe_writer = None;
    for (module_name, module_config) in config.modules {
        let mid = ModuleId(module_config.id);
        ldebug!("Initializing module {}: {:?}", module_name, module_config);

        let (events_write, events_read) = channel::bounded(EV_CHANNEL_SIZE);
        let (command_write, command_read) = channel::bounded(1);
        last_pipe_writer = Some(events_write.clone());
        let common = InputCommon {
            needs_reset: false,
            running: immediate_start,
            module: mid,
            events: events_write,
            state: state_write.clone(),
            command: command_read,
            command_reply: cmd_reply_write.clone(),
            recipe: recipe::from_config(&config.recipes, &module_config.recipe)?,
        };
        event_read_chans.push(events_read);
        command_write_chans.insert(mid, command_write);

        if let Err(e) = input::start(module_config.specific, common) {
            lprintln!(ERROR, "Failed to initialize module {}: {}", module_name, e);
            init_errors = true;
        }
    }
    if init_errors {
        Err(anyhow!("Some modules failed to initialize"))?;
    }

    // create event sorters if we have more than one module
    while event_read_chans.len() > 1 {
        let read_1 = event_read_chans.pop().expect("len checked");
        let read_2 = event_read_chans.pop().expect("len checked");
        let (write, sorted_read) = channel::bounded(EV_CHANNEL_SIZE);
        last_pipe_writer = Some(write.clone());
        sorter::Sorter::new(read_1, read_2, write).start()?;
        event_read_chans.push(sorted_read);
    }

    // the last remaining channel gets all events, sorted
    let events_read = event_read_chans.pop().expect("len checked");
    let to_postproc = last_pipe_writer.expect("len checked");

    // create command handler
    drop(state_write);
    drop(cmd_reply_write);
    command::CommandHandler::start(
        if_cmd_read,
        command_write_chans,
        cmd_reply_read,
        if_reply_write,
        to_postproc.clone(),
    )?;

    let post_recipe = recipe::from_config(&config.recipes, &config.postprocess.recipe)?;

    let (x, _) = channel::bounded(1);
    let postproc = postproc::PostProcessor::new(post_recipe, events_read, x, shm, config.histogram);
    postproc.start()?;

    uds.start()?;

    while let Ok(state) = state_read.recv() {
        lprintln!(INFO, "Module state: {:?}", state);
        // Send update to the postprocess/shm thread to update clients
        to_postproc.send(PipeItem::State(state))
                   .expect("postprocessor command receiver died");
        // TODO more?
    }

    Ok(())
}
