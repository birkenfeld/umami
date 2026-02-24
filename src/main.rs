// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

#![allow(dead_code)]

mod config;
mod error;
mod event;
mod input;
mod util;

use std::path::PathBuf;
use clap::Parser;

#[derive(Parser)]
#[clap(version, author, about)]
pub struct Options {
    #[clap(long="config", default_value="umami.conf", help="Config file")]
    config: PathBuf,
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

fn main() {
    let args = Options::parse();
    let config: config::Config = match std::fs::read_to_string(&args.config) {
        Ok(s) => {
            toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("Failed to parse config file {}: {}", args.config.display(), e);
                std::process::exit(1);
            })
        }
        Err(e) => {
            eprintln!("Failed to read config file {}: {}", args.config.display(), e);
            std::process::exit(1);
        }
    };

    let mut modules = Vec::new();
    let mut module_id = 0;
    for (module_name, module_config) in config.modules {
        println!("Configured module {}: {:?}", module_name, module_config);
        let module = match input::create_input(event::ModuleId(module_id), module_config) {
            Ok(i) => i,
            Err(e) => {
                // TODO: should be non-fatal? How/when to reinitialize?
                eprintln!("Failed to initialize module {}: {}", module_name, e);
                std::process::exit(1);
            }
        };
        println!("Initialized {}: {}", module_name, module.description());
        modules.push(module);
        module_id += 1;
    }

    for mut module in modules {
        std::thread::spawn(move || {
            while let Ok(ev) = input::Input::read_event(&mut *module) {
                // println!("Received event: {:?}", ev);
            }
        });
    }

    loop {
        std::thread::park();
    }
}
