// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::path::PathBuf;
use clap::Parser;

use umami::lprintln;

#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Parser)]
#[clap(version, author, about)]
pub struct Options {
    #[clap(default_value="umami.conf", help="Configuration to use")]
    config: PathBuf,
    #[clap(long="start", help="Start the pipeline immediately")]
    start: bool,
    #[clap(long="ipc", help="Name of the instance for IPC, overrides config file")]
    ipc_name: Option<String>,
    #[clap(long="debug", help="Enable debug output")]
    debug: bool,
    #[clap(long="trace", help="Enable trace output (every event)")]
    trace: bool,
}

fn inner_main(args: Options) -> umami::UResult<()> {
    lprintln!(INFO, "Starting UMAMI with config file {:?}", args.config.display());
    let mut config = umami::load_config(&args.config)?;

    if let Some(ipc_name) = args.ipc_name {
        config.ipc_name = ipc_name.to_string();
    }

    umami::set_debug_params(config.debug | args.debug | args.trace, args.trace)?;
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
