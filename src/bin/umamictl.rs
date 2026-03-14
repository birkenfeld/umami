// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::os::unix::net;
use std::time::Duration;
use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use serde_json::value;
use uds::UnixDatagramExt;
use umami::{Command, CommandReply};

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
    /// Enable and set the path to the raw dump files.
    Raw { path: String },
    /// Disable raw dump files.
    NoRaw,
    /// Get all possible mode names.
    Modes,
    /// Set mode name and parameters for histogramming.
    Mode { name: String, params: value::Map<String, value::Value> },
    /// Get the current state.
    State,
}

fn inner_main(args: Options) -> anyhow::Result<()> {
    let unique_name = format!("umamictl-{}", std::process::id());
    let my_addr = uds::UnixSocketAddr::from_abstract(&unique_name)
        .context("Creating abstract socket address")?;
    let target_addr = uds::UnixSocketAddr::from_abstract(args.ipc_name.as_bytes())
        .context("Creating abstract socket address")?;
    let sock = net::UnixDatagram::bind_unix_addr(&my_addr)
        .context("Creating Unix socket")?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .context("Setting socket read timeout")?;
    sock.connect_to_unix_addr(&target_addr).context("Connecting Unix socket")?;

    let cmd = match args.cmd {
        Cmd::Start { run_id } => Command::Start { run_id },
        Cmd::Stop => Command::Stop,
        Cmd::Clear => Command::Clear,
        Cmd::Raw { path } => Command::SetRawDump { enable: true, path },
        Cmd::NoRaw => Command::SetRawDump { enable: false, path: String::new() },
        Cmd::Mode { name, params } =>
            Command::SetMode { name, params: toml::Table::try_from(params)? },
        Cmd::Modes => Command::GetModes,
        Cmd::State => Command::GetState,
    };

    let cmd_json = serde_json::to_string(&cmd).context("Serializing command to JSON")?;
    sock.send(cmd_json.as_bytes()).context("Sending command")?;
    let mut buf = [0; 4096];
    let n = match sock.recv(&mut buf) {
        Ok(n) => n,
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => bail!("No reply received (timeout)"),
        Err(e) if e.kind() == io::ErrorKind::TimedOut => bail!("No reply received (timeout)"),
        Err(e) => Err(e).context("Receiving reply")?,
    };
    let reply_buf = str::from_utf8(&buf[..n])?;
    let reply: CommandReply = serde_json::from_str(reply_buf)
        .context("Parsing reply JSON")?;
    match reply {
        CommandReply::Ok => println!("OK"),
        CommandReply::Error { module, message } => {
            let module_str = module.map_or("".to_string(), |m| format!("Module {}: ", m.0));
            bail!("{}{}", module_str, message);
        },
        CommandReply::Data { module, value } => {
            if let Some(module) = module {
                println!("Module {}: {}", module.0, value);
            } else {
                println!("{}", value);
            }
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
