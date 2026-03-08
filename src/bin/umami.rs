// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::sync::atomic::Ordering;
use std::path::PathBuf;
use anyhow::Context;
use clap::Parser;

use umami::{lprintln, DEBUG, TRACE};

#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Parser)]
#[clap(version, author, about)]
pub struct Options {
    #[clap(default_value="umami.conf", help="Configuration to use")]
    config: PathBuf,
    #[clap(long="start", help="Start the pipeline immediately")]
    start: bool,
    #[clap(long="ipc-name", help="Name of the SHM and UDS interface, overrides config file")]
    ipc_name: Option<String>,
    #[clap(long="debug", help="Enable debug output")]
    debug: bool,
    #[clap(long="trace", help="Enable trace output (every event)")]
    trace: bool,
}

fn inner_main(args: Options) -> umami::UResult<()> {
    lprintln!(INFO, "Starting UMAMI with config file {:?}", args.config.display());
    let mut config: umami::Config = toml::from_str(
        &std::fs::read_to_string(&args.config)
            .with_context(|| format!("Failed to read config file {:?}", args.config.display()))?
    ).with_context(|| format!("Failed to parse config file {:?}", args.config.display()))?;

    let n_modules = config.modules.len();
    if n_modules == 0 {
        lprintln!(ERROR, "No modules configured, exiting.");
        std::process::exit(1);
    }

    DEBUG.store(config.debug | args.debug | args.trace, Ordering::Relaxed);
    TRACE.store(args.trace, Ordering::Relaxed);

    if let Some(ipc_name) = args.ipc_name {
        config.ipc_name = ipc_name.to_string();
    }

    umami::run_pipeline(config, args.start)
}

fn main() {
    let args = Options::parse();
    match inner_main(args) {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            lprintln!(FATAL, "Exiting due to init error: {e:#}");
            std::process::exit(1);
        }
    }
}
