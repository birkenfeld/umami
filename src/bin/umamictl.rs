// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::io;
use std::os::unix::net;
use std::time::Duration;
use anyhow::{bail, Context};
use clap::Parser;
use uds::UnixDatagramExt;
use umami::{Command, CommandReply};

#[derive(Parser)]
#[clap(version, author, about)]
pub struct Options {
    #[clap(long="ipc", default_value="umami",
           help="Name of the instance for IPC")]
    ipc_name: String,
    #[clap(help="Command to send")]
    command: String,
    #[clap(help="Command argument")]
    arg: Option<String>,
}

// TODO subcommands

fn inner_main(args: Options) -> anyhow::Result<()> {
    // TODO: needs a unique name
    let my_addr = uds::UnixSocketAddr::from_abstract("umamictl")
        .context("Creating abstract socket address")?;
    let target_addr = uds::UnixSocketAddr::from_abstract(args.ipc_name.as_bytes())
        .context("Creating abstract socket address")?;
    let sock = net::UnixDatagram::bind_unix_addr(&my_addr)
        .context("Creating Unix socket")?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .context("Setting socket read timeout")?;
    sock.connect_to_unix_addr(&target_addr).context("Connecting Unix socket")?;

    let cmd = match &*args.command {
        "start" => Command::Start { run_id: args.arg.expect("cmdarg") }, // TODO
        "stop" => Command::Stop,
        "clear" => Command::Clear,
        "raw" => Command::SetRawDump {
            enable: true,
            path: args.arg.expect("cmdarg"),
        },
        other => bail!("Unknown command: {other}"),
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
        CommandReply::Data { .. } => bail!("Unexpected data reply"),
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
