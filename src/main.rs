// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod config;
mod error;
mod event;
mod input;
mod interface;
mod util;

use std::sync::atomic::{AtomicBool, Ordering};
use std::path::PathBuf;
use anyhow::Context;
use clap::Parser;

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

    DEBUG.store(config.debug | args.debug | args.trace, Ordering::Relaxed);
    TRACE.store(args.trace, Ordering::Relaxed);

    let mut shm = interface::ShmInterface::map(&config.shm_name)?;
    shm.initialize();

    let (events_write, events_read) = channel::bounded(1024);  // TODO tune capacity
    let (state_write, state_read) = channel::bounded(16);
    let (_command_write, command_read) = channel::bounded(16);
    let (_config_request_write, config_request_read) = channel::bounded(16);
    let (config_reply_write, _config_reply_read) = channel::bounded(16);
    let channels = input::InputChannels {
        events: events_write,
        state: state_write,
        command: command_read,
        config_request: config_request_read,
        config_reply: config_reply_write,
    };

    let n_modules = config.modules.len();

    for (module_id, (module_name, module_config)) in config.modules.into_iter().enumerate() {
        ldebug!("Initializing module {}: {:?}", module_name, module_config);
        if let Err(e) = input::init(event::ModuleId(module_id as _), module_config, channels.clone()) {
            // TODO: should be non-fatal? How/when to reinitialize?
            lprintln!(ERROR, "Failed to initialize module {}: {}", module_name, e);
            std::process::exit(1);
        }
    }
    drop(channels);  // drop unused channels in main thread

    std::thread::spawn(move || {
        let successful = state_read.take(n_modules).count();
        lprintln!(INFO, "All modules finished, {} successful", successful);
        std::process::exit(0);
    });

    let mut i: usize = 0;
    let mut limit = 0;
    for ev in events_read {
        i += ev.len();
        if i > limit {
            println!("Received {} events", i);
            limit += 1000000;
        }
    }

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
