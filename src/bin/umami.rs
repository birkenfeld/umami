// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::sync::atomic::Ordering;
use std::path::PathBuf;
use anyhow::Context;
use clap::Parser;

use umami::{channel, recipe, ldebug, lprintln, DEBUG, TRACE};
use umami::config::Config;
use umami::error::UResult;
use umami::event::{EventData, EventTime, ModuleId};
use umami::histo::Histogram;
use umami::input::{self, InputPlumbing};
use umami::interface::ShmInterface;
use umami::sorter::Sorter;

#[derive(Parser)]
#[clap(version, author, about)]
pub struct Options {
    #[clap(long="config", default_value="umami.conf", help="Config file")]
    config: PathBuf,
    #[clap(long="debug", default_value="false", help="Enable debug output")]
    debug: bool,
    #[clap(long="trace", default_value="false", help="Enable trace output (every event)")]
    trace: bool,
}

fn inner_main(args: Options) -> UResult<()> {
    let config: Config = toml::from_str(
        &std::fs::read_to_string(&args.config)
            .with_context(|| format!("Failed to read config file {}", args.config.display()))?
    ).with_context(|| format!("Failed to parse config file {}", args.config.display()))?;

    let n_modules = config.modules.len();
    if n_modules == 0 {
        lprintln!(ERROR, "No modules configured, exiting.");
        std::process::exit(1);
    }

    DEBUG.store(config.debug | args.debug | args.trace, Ordering::Relaxed);
    TRACE.store(args.trace, Ordering::Relaxed);

    let mut shm = ShmInterface::map(&config.shm_name)?;
    shm.initialize();

    const EV_BOUND: usize = 64; // TODO tune
    let (state_write, _state_read) = channel::bounded(16);
    let (_command_write, command_read) = channel::bounded(16);
    let (_config_request_write, config_request_read) = channel::bounded(16);
    let (config_reply_write, _config_reply_read) = channel::bounded(16);

    let start = jiff::Timestamp::now();

    let mut event_read_chans = vec![];
    for (module_name, module_config) in config.modules {
        ldebug!("Initializing module {}: {:?}", module_name, module_config);

        let (events_write, events_read) = channel::bounded(EV_BOUND);
        let plumbing = InputPlumbing {
            events: events_write,
            state: state_write.clone(),
            command: command_read.clone(),
            config_request: config_request_read.clone(),
            config_reply: config_reply_write.clone(),
            recipe: recipe::from_config(&config.recipes, &module_config.recipe)?,
        };
        event_read_chans.push(events_read);

        if let Err(e) = input::init(ModuleId(module_config.id),
                                    module_config.specific, plumbing) {
            // TODO: should be non-fatal? How/when to reinitialize?
            lprintln!(ERROR, "Failed to initialize module {}: {}", module_name, e);
            std::process::exit(1);
        }
    }

    // create event sorters if we have more than one module
    while event_read_chans.len() > 1 {
        let read_1 = event_read_chans.pop().expect("len checked");
        let read_2 = event_read_chans.pop().expect("len checked");
        let (write, sorted_read) = channel::bounded(EV_BOUND);
        Sorter::run(read_1, read_2, write);
        event_read_chans.push(sorted_read);
    }

    // the last remaining channel gets all events, sorted
    let events_read = event_read_chans.pop().expect("len checked");

    // drop unused channels
    drop(state_write);
    drop(command_read);
    drop(config_request_read);
    drop(config_reply_write);

    let mut post_recipe = recipe::from_config(&config.recipes, &config.postprocess.recipe)?;
    let mut histo = Histogram::new(config.histogram.nx, config.histogram.ny);

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

fn main() {
    let args = Options::parse();
    match inner_main(args) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}
