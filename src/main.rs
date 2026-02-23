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

    let mut inputs = Vec::new();
    let mut module_id = 0;
    for (input_name, input_config) in config.inputs {
        println!("Input {}: {:?}", input_name, input_config);
        let input = match input::create_input(event::ModuleId(module_id), input_config) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("Failed to initialize input {}: {}", input_name, e);
                std::process::exit(1);
            }
        };
        inputs.push(input);
        module_id += 1;
    }

    for mut input in inputs {
        std::thread::spawn(move || {
            while let Ok(ev) = input::Input::read_event(&mut *input) {
                println!("Received event: {:?}", ev);
            }
        });
    }

    loop {
        std::thread::park();
    }
}
