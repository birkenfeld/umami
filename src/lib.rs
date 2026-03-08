// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod command;
mod config;
mod error;
mod event;
mod histo;
mod input;
mod interface;
mod pipeline;
mod recipe;
mod sorter;
mod util;

use kanal as channel;

use std::sync::atomic::AtomicBool;

pub static DEBUG: AtomicBool = AtomicBool::new(false);
// TODO: does nothing right now, add cfg for this feature and log
pub static TRACE: AtomicBool = AtomicBool::new(false);

// Public API
pub use self::command::{Command, CommandReply};
pub use self::config::Config;
pub use self::error::UResult;
pub use self::pipeline::run_pipeline;

#[macro_export]
macro_rules! ldebug {
    ($($tt:tt)+) => {
        if $crate::DEBUG.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : DEBUG : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.f"));
            eprintln!($($tt)+);
        }
    };
}

#[macro_export]
macro_rules! ltrace {
    ($($tt:tt)+) => {
        if $crate::TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprint!("{} : TRACE : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"));
            eprintln!($($tt)+);
        }
    };
}

#[macro_export]
macro_rules! lprintln {
    ($lvl:expr, $($tt:tt)+) => {
        eprint!("{} : {:-5} : ", jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S%.9f"),
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
