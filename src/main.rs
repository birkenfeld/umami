// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod config;
mod error;
mod event;
mod histo;
mod input;
mod interface;
mod recipe;
mod sorter;
mod util;

use std::sync::atomic::{AtomicBool, Ordering};
use std::path::PathBuf;
use anyhow::Context;
use clap::Parser;

use event::EventTime;
pub use kanal as channel;

static DEBUG: AtomicBool = AtomicBool::new(false);
static TRACE: AtomicBool = AtomicBool::new(false);

#[macro_export]
macro_rules! ldebug {
    ($($tt:tt)+) => {
        if crate::DEBUG.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : DEBUG : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"));
            eprintln!($($tt)+);
        }
    };
}

#[macro_export]
macro_rules! ltrace {
    ($($tt:tt)+) => {
        if crate::TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : TRACE : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"));
            eprintln!($($tt)+);
        }
    };
}

#[macro_export]
macro_rules! lprintln {
    ($lvl:expr, $($tt:tt)+) => {
        eprint!("{} : {:-5} : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"),
                stringify!($lvl));
        eprintln!($($tt)+);
    };
}

#[macro_export]
macro_rules! lpanic {
    ($($tt:tt)+) => {
        { lprintln!(ERROR, $($tt)+); std::process::exit(1); }
    };
}

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

// TODO Pipeline draft
//
//   - input: read raw events, translate to internal event format
//
// / - raw: write raw data to disk (TODO use original or internal format?)
// | - signals: assign meaning to additional events
// | - translate: apply actual setup from instrument to calculate pixel
// \   x,y from amplitudes, calibration, mapping tables, offsets
//
//   - filter: remove unneeded events?
//   - tof: calculate time-of-flight from chopper/t0 events
//
//   - histogram: accumulate events into histograms
//
// / - output: write to disk in some format (e.g. NeXus)
// \ - live: provide live view for Tango server

fn inner_main(args: Options) -> error::UResult<()> {
    let config: config::Config = toml::from_str(
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

    let mut shm = interface::ShmInterface::map(&config.shm_name)?;
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
        let plumbing = input::InputPlumbing {
            events: events_write,
            state: state_write.clone(),
            command: command_read.clone(),
            config_request: config_request_read.clone(),
            config_reply: config_reply_write.clone(),
            recipe: recipe::from_config(&config.recipes, &module_config.recipe)?,
        };
        event_read_chans.push(events_read);

        if let Err(e) = input::init(event::ModuleId(module_config.id),
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
        sorter::Sorter::run(read_1, read_2, write);
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
                event::EventData::Neutron { x, y, .. } => {
                    histo.add(x as usize, y as usize);
                }
                _ => {}
            }
        }
    }

    let stop = jiff::Timestamp::now();
    println!("Final count: {} events in {} secs, {} out of order", i, stop - start, ooo);

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
