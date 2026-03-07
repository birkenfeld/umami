// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::anyhow;
use crate::{channel, lprintln, ldebug};
use crate::{command, histo, input, recipe, sorter};
use crate::config::Config;
use crate::error::UResult;
use crate::event::{EventData, EventTime, ModuleId};
use crate::input::InputPlumbing;
use crate::interface::{UdsInterface, ShmInterface};


pub fn run_pipeline(config: Config, ipc_name: Option<&str>) -> UResult<()> {
    let ipc_name = ipc_name.unwrap_or(&config.ipc_name);
    let mut shm = ShmInterface::map(ipc_name)?;
    shm.reset();

    let (if_cmd_write, if_cmd_read) = channel::bounded(16);
    let (if_reply_write, if_reply_read) = channel::bounded(16);
    let uds = UdsInterface::new(ipc_name, if_cmd_write, if_reply_read)?;

    lprintln!(INFO, "IPC interfaces are available under the name {:?}", ipc_name);

    const EV_BOUND: usize = 64; // TODO tune
    let (state_write, state_read) = channel::bounded(16);
    let (cmd_reply_write, cmd_reply_read) = channel::bounded(16);

    let start = jiff::Timestamp::now();

    let mut init_errors = false;
    let mut event_read_chans = vec![];
    let mut command_write_chans = BTreeMap::new();
    for (module_name, module_config) in config.modules {
        ldebug!("Initializing module {}: {:?}", module_name, module_config);

        let (events_write, events_read) = channel::bounded(EV_BOUND);
        let (command_write, command_read) = channel::bounded(32);
        let plumbing = InputPlumbing {
            events: events_write,
            state: state_write.clone(),
            command: command_read,
            command_reply: cmd_reply_write.clone(),
            recipe: recipe::from_config(&config.recipes, &module_config.recipe)?,
        };
        event_read_chans.push(events_read);
        command_write_chans.insert(ModuleId(module_config.id), command_write);

        if let Err(e) = input::init(ModuleId(module_config.id),
                                    module_config.specific, plumbing) {
            lprintln!(ERROR, "Failed to initialize module {}: {}", module_name, e);
            init_errors = true;
        }
    }
    if init_errors {
        Err(anyhow!("Exiting due to module init errors"))?;
    }

    // create event sorters if we have more than one module
    while event_read_chans.len() > 1 {
        let read_1 = event_read_chans.pop().expect("len checked");
        let read_2 = event_read_chans.pop().expect("len checked");
        let (write, sorted_read) = channel::bounded(EV_BOUND);
        sorter::Sorter::run(read_1, read_2, write)?;
        event_read_chans.push(sorted_read);
    }

    // the last remaining channel gets all events, sorted
    let events_read = event_read_chans.pop().expect("len checked");

    // create command handler
    drop(state_write);
    drop(cmd_reply_write);
    command::CommandHandler::run(if_cmd_read, command_write_chans, cmd_reply_read, if_reply_write)?;

    std::thread::spawn(move || {
        while let Ok(state) = state_read.recv() {
            lprintln!(INFO, "Module state: {:?}", state);
        }
    });

    let mut post_recipe = recipe::from_config(&config.recipes, &config.postprocess.recipe)?;

    uds.run()?;

    let mut histo = histo::Histogram::new(config.histogram.nx, config.histogram.ny);

    let mut i: usize = 0;
    let mut limit = 0;
    let mut ts = EventTime::zero();
    let mut ooo = 0;
    for mut evs in events_read {
        i += evs.len();
        evs = post_recipe.process(evs);
        if i > limit {
            println!("Received {} events", i);
            limit += 1000000;
        }
        for ev in evs {
            let nts = ev.time;
            if nts < ts {
                ooo += 1;
            }
            ts = nts;

            match ev.data {
                EventData::Neutron { x, y, .. } => {
                    histo.add(x as usize, y as usize);
                }
                _ => {}
            }
        }
    }

    let stop = jiff::Timestamp::now();
    println!("Final count: {} events in {} secs, {} out of order", i, stop - start, ooo);

    histo.plot();

    Ok(())
}
