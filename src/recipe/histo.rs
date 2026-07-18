// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventData, EventFlags, EventTime};
use crate::params::HasParams;
use crate::recipe::Recipe;

#[derive(HasParams)]
pub struct Tof {
    // Configuration
    /// Binning in spatial directions.
    #[param(help="Binning in x direction")]
    bin_x: u32,
    #[param(help="Binning in y direction")]
    bin_y: u32,
    /// Whether to use the gate signal to filter events. If false, gate signals
    /// are ignored.
    #[param(help="Whether to use gate signal")]
    use_gate: bool,
    /// If set, this is the value of the AuxSignal that is used as T0. If None,
    /// T0 is given by Tzero events.
    #[param(help="Aux signal number to use as T0, or false to use explicit T0 events",
            datatype="null or integer")]
    aux_mode: Option<u32>,
    /// Contains end of time bins since T0.
    #[param(help="Time binning end times (first bin always starts at offset 0)",
            datatype="array of integers (nanoseconds)")]
    time_bins: Vec<EventTime>,
    // Run-time state
    gate_up: bool,
    last_t0: EventTime,
    cur_bin: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TofConfig {
    pub bin_x: Option<u32>,
    pub bin_y: Option<u32>,
    pub use_gate: Option<bool>,
    pub aux_mode: Option<u32>,
    pub time_bins: Option<Vec<EventTime>>,
}

impl Recipe for Tof {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let config: TofConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        Ok(Tof {
            bin_x: config.bin_x.unwrap_or(1),
            bin_y: config.bin_y.unwrap_or(1),
            use_gate: config.use_gate.unwrap_or(false),
            aux_mode: config.aux_mode,
            time_bins: config.time_bins.unwrap_or_default(),
            gate_up: false,
            last_t0: EventTime::zero(),
            cur_bin: 0
        })
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::Tzero => {
                    if self.aux_mode.is_none() {
                        self.last_t0 = event.time;
                        self.cur_bin = 0;
                    }
                }
                EventData::AuxSignal { num } => {
                    if self.aux_mode == Some(num as _) {
                        self.last_t0 = event.time;
                        self.cur_bin = 0;
                    }
                }
                EventData::Gate { up } => self.gate_up = up,
                _ => {
                    if self.use_gate && !self.gate_up {
                        event.data = EventData::Void;
                        continue;
                    }

                    if !event.flags.contains(EventFlags::HasRelTime) {
                        event.rel_time = event.time - self.last_t0;
                        event.flags.set(EventFlags::HasRelTime);
                    }

                    if let EventData::Neutron = event.data {
                        if self.time_bins.len() > 1 {
                            // find the correct bin for this relative time
                            // this can never overflow, since the final bin is guaranteed
                            // to be the EventTime::MAX
                            while event.rel_time >= self.time_bins[self.cur_bin] {
                                self.cur_bin += 1;
                            }
                            event.t = self.cur_bin as u32;
                        }
                        event.x /= self.bin_x;
                        event.y /= self.bin_y;
                    }
                }
            }
        }
        events
    }
}


#[derive(HasParams)]
pub struct Std {
    // Configuration
    /// Binning in spatial directions.
    #[param(help="Binning in x direction")]
    bin_x: u32,
    #[param(help="Binning in y direction")]
    bin_y: u32,
    /// Whether to use the gate signal to filter events. If false, gate signals
    /// are ignored.
    #[param(help="Whether to use gate signal")]
    use_gate: bool,

    // Run-time state
    gate_up: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StdConfig {
    pub bin_x: Option<u32>,
    pub bin_y: Option<u32>,
    pub use_gate: Option<bool>,
}

impl Recipe for Std {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let config: StdConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        Ok(Std {
            bin_x: config.bin_x.unwrap_or(1),
            bin_y: config.bin_y.unwrap_or(1),
            use_gate: config.use_gate.unwrap_or(false),
            gate_up: false,
        })
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::Gate { up } => self.gate_up = up,
                _ => {
                    if self.use_gate && !self.gate_up {
                        event.data = EventData::Void;
                        continue;
                    }

                    if let EventData::Neutron = event.data {
                        event.x /= self.bin_x;
                        event.y /= self.bin_y;
                    }
                }
            }
        }
        events
    }
}
