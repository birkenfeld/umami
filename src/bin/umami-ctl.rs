// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::bail;
use clap::{Parser, Subcommand};
use umami::{Client, Command, CommandReply, ParamMap};

#[derive(Parser)]
#[clap(disable_help_subcommand = true)]
#[clap(version, author, about)]
/// A utility for managing UMAMI data acquisition.
pub struct Options {
    #[clap(long="ipc", default_value="umami")]
    /// Name of the instance for IPC.
    ipc_name: String,
    #[clap(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Start a new run with the given run ID.  Does not clear the histogram!
    Start { run_id: String },
    /// Stop the current run.
    Stop,
    /// Clear the histogram.  Can be called while running.
    Clear,
    /// Reset the input modules.  Does not clear the histogram.
    Reset,
    /// Save the current histogram to the given path, can be called while running
    SaveHisto { path: String, max_nt: usize },
    /// Enable and set the path to the raw dump files.
    Raw { path: String },
    /// Disable raw dump files.
    NoRaw,
    /// Get mode names for postprocessing.
    GetModes,
    /// Set mode name for postprocessing.
    SetMode { name: String },
    /// Get available parameters and their current values.
    GetParams {
        /// Include datatype/help/readonly metadata and each module's _info entry.
        #[clap(long)]
        full: bool,
    },
    /// Set some parameter values.
    SetParams { params: ParamMap },
    /// Get the current state.
    State,
    /// Get Umami version.
    Ping,
}

fn inner_main(args: Options) -> anyhow::Result<()> {
    let client = Client::new(&args.ipc_name)?;

    let cmd = match args.cmd {
        Cmd::Start { run_id } => Command::Start { run_id },
        Cmd::Stop => Command::Stop,
        Cmd::Clear => Command::Clear,
        Cmd::Reset => Command::Reset,
        Cmd::SaveHisto { path, max_nt } => Command::SaveHisto { path, max_nt },
        Cmd::Raw { path } => Command::SetRawDump { enable: true, path },
        Cmd::NoRaw => Command::SetRawDump { enable: false, path: String::new() },
        Cmd::GetModes => Command::GetModes,
        Cmd::SetMode { name } => Command::SetMode { name },
        Cmd::GetParams { full } => Command::GetParams { full },
        Cmd::SetParams { params } => Command::SetParams { params },
        Cmd::State => Command::GetState,
        Cmd::Ping => Command::Ping,
    };

    let reply = client.send(&cmd)?;
    match reply {
        CommandReply::Ok => println!("OK"),
        CommandReply::Error { module, message } => {
            let module_str = module.map_or(String::new(), |m| format!("Module {m}: "));
            bail!("{module_str}{message}");
        },
        CommandReply::Data { value } => {
            println!("{value}");
        }
    }

    Ok(())
}

fn main() {
    let args = Options::parse();
    match inner_main(args) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}
