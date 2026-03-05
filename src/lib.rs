// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

pub mod config;
pub mod error;
pub mod event;
pub mod histo;
pub mod input;
pub mod interface;
pub mod recipe;
pub mod sorter;
mod util;

pub use kanal as channel;

use std::sync::atomic::AtomicBool;

pub static DEBUG: AtomicBool = AtomicBool::new(false);
pub static TRACE: AtomicBool = AtomicBool::new(false);

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
