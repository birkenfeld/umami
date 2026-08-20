// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Various binary formats for output and IPC.

use anyhow::anyhow;
use zerocopy::{Immutable, IntoBytes, FromBytes};
use crate::error::UResult;
use crate::event::Event;

pub trait Format : Immutable + IntoBytes + FromBytes + Send + Sync + 'static {
    const NAME: &'static str;
    #[allow(dead_code, reason = "not yet used")]
    const DESCRIPTION: &'static str;

    /// Convert an Event into this format.
    fn from_event(event: Event) -> Self;
}

/// Reads the required `format` config key.
pub fn format_name(config: &toml::Table) -> UResult<&str> {
    let value = config.get("format")
        .ok_or_else(|| anyhow!("Missing 'format' in output config"))?;
    Ok(value.as_str()
        .ok_or_else(|| anyhow!("'format' in output config must be a string"))?)
}

/// Dispatches on a format name (as accepted in config, matching some
/// `Format::NAME`) to run `$body` with `$f` bound as a type alias for the
/// matching `Format` impl. Add a new arm here when adding a new `Format`.
#[macro_export]
macro_rules! with_format {
    ($name:expr, |$f:ident| $body:expr) => {
        match $name {
            $crate::format::Full::NAME => { type $f = $crate::format::Full; $body }
            $crate::format::XYTof::NAME => { type $f = $crate::format::XYTof; $body }
            other => Err(anyhow::anyhow!(
                "Unknown format {other:?}, available: {:?}, {:?}",
                $crate::format::Full::NAME, $crate::format::XYTof::NAME).into()),
        }
    };
}

/// The full event format, including all fields.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Immutable, IntoBytes, FromBytes)]
pub struct Full {
    pub evtype: u8,
    pub index: u8,
    pub flags: u16,
    pub channel: u32,
    pub raw: [u32; 2],
    pub ampl: u32,
    pub reserve: u32,
    pub histo: [u16; 4],
    pub time: i64,
    pub rel_time: i64,
}

impl Format for Full {
    const NAME: &'static str = "full";
    const DESCRIPTION: &'static str = "The full event format, including all fields";

    fn from_event(event: Event) -> Self {
        Full {
            evtype: event.evtype as u8,
            index: event.index,
            flags: u16::from(event.flags),
            channel: event.channel.0,
            raw: event.raw,
            ampl: event.ampl.0,
            reserve: event.reserve,
            histo: [event.histo.x, event.histo.y, event.histo.t, event.histo.i],
            time: event.time.0,
            rel_time: event.rel_time.0,
        }
    }
}


/// Only the x, y coordinates and time-of-flight in 100ns units.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Immutable, IntoBytes, FromBytes)]
pub struct XYTof {
    pub x: u16,
    pub y: u16,
    pub tof: u32,
}

impl Format for XYTof {
    const NAME: &'static str = "xy_tof";
    const DESCRIPTION: &'static str = "Only the x, y coordinates and time-of-flight in 100ns units";

    fn from_event(event: Event) -> Self {
        XYTof {
            x: event.histo.x,
            y: event.histo.y,
            tof: (event.rel_time.0 / 100) as u32
        }
    }
}
