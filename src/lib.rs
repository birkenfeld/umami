// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod client;
mod command;
mod config;
mod error;
mod event;
mod expr;
pub mod format;
mod input;
mod output;
mod params;
mod pipeline;
mod postproc;
mod recipe;
mod shm;
mod sorter;
mod util;

use flume as channel;

use anyhow::anyhow;
use std::sync::atomic::{AtomicBool, Ordering};

// Public API
pub use self::client::{Client, ClientError};
pub use self::command::{Command, CommandReply, RECV_BUFFER_SIZE};
pub use self::config::{load_config, HistoConfig};
pub use self::error::UResult;
pub use self::event::{Event, EventTime, EventType};
pub use self::output::ext_process::FrameTag;
pub use self::params::ParamMap;
pub use self::pipeline::{start_pipeline, PipelineHandle};
pub use self::shm::{ShmReader, ShmWriter, RUNNING_BIT, SHM_MAGIC};
pub use self::util::wait_for_signal;

static DEBUG: AtomicBool = AtomicBool::new(false);
static TRACE: AtomicBool = AtomicBool::new(false);

pub fn set_debug_params(debug: bool, trace: bool) -> UResult<()> {
    DEBUG.store(debug, Ordering::Relaxed);
    if cfg!(feature = "trace") {
        TRACE.store(trace, Ordering::Relaxed);
    } else if trace {
        Err(anyhow!("Trace logging is not available in this build"))?;
    }
    Ok(())
}

#[macro_export]
macro_rules! ldebug {
    ([$mod:expr] $($tt:tt)+) => {{
        if $crate::DEBUG.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : DEBUG : [{}] ",
                    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"),
                    $mod);
            eprintln!($($tt)+);
        }
    }};
    ($($tt:tt)+) => {{
        if $crate::DEBUG.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : DEBUG : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"));
            eprintln!($($tt)+);
        }
    }};
}

#[macro_export]
macro_rules! ltrace {
    ([$mod:expr] $($tt:tt)+) => {{
        #[cfg(feature = "trace")]
        if $crate::TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : TRACE : [{}] ",
                    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"),
                    $mod);
            eprintln!($($tt)+);
        }
    }};
    ($($tt:tt)+) => {{
        #[cfg(feature = "trace")]
        if $crate::TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : TRACE : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"));
            eprintln!($($tt)+);
        }
    }};
}

#[macro_export]
macro_rules! lprintln {
    ($lvl:expr, [$mod:expr] $($tt:tt)+) => {{
        eprint!("{} : {:-5} : [{}] ",
                jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"),
                stringify!($lvl),
                $mod);
        eprintln!($($tt)+);
    }};
    ($lvl:expr, $($tt:tt)+) => {{
        eprint!("{} : {:-5} : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"),
                stringify!($lvl));
        eprintln!($($tt)+);
    }};
}
