// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventData, EventFlags, EventTime};
use crate::recipe::Recipe;
use crate::util::FalseOr;

pub struct TofStd {
    // Configuration
    /// Whether to use the gate signal to filter events. If false, gate signals
    /// are ignored.
    use_gate: bool,
    /// If set, this is the value of the AuxSignal that is used as T0. If None,
    /// T0 is given by Tzero events.
    aux_mode: FalseOr<u32>,
    /// Contains end of time bins since T0.
    time_bins: Vec<EventTime>,
    // Run-time state
    gate_up: bool,
    last_t0: EventTime,
    cur_bin: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TofConfig {
    pub use_gate: Option<bool>,
    pub aux_mode: Option<FalseOr<u32>>,
    pub time_bins: Option<Vec<f64>>,
}

impl Recipe for TofStd {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let config: TofConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        Ok(TofStd {
            use_gate: config.use_gate.unwrap_or(false),
            aux_mode: config.aux_mode.unwrap_or(FalseOr::False),
            time_bins: config.time_bins.unwrap_or_default()
                                       .into_iter()
                                       .map(EventTime::from_floating_sec)
                                       .chain(Some(EventTime::MAX))
                                       .collect(),
            gate_up: false,
            last_t0: EventTime::zero(),
            cur_bin: 0
        })
    }

    fn update_config(&mut self, config: toml::Table) -> UResult<()> {
        let config: TofConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        if let Some(use_gate) = config.use_gate {
            self.use_gate = use_gate;
        }
        if let Some(aux_mode) = config.aux_mode {
            self.aux_mode = aux_mode;
        }
        if let Some(time_bins) = config.time_bins {
            self.time_bins = time_bins.into_iter()
                                      .map(EventTime::from_floating_sec)
                                      .chain(Some(EventTime::MAX))
                                      .collect();
        }
        Ok(())
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.data {
                EventData::Tzero => {
                    if self.aux_mode.is_false() {
                        self.last_t0 = event.time;
                        self.cur_bin = 0;
                    }
                }
                EventData::AuxSignal { number, up: true } => {
                    if self.aux_mode == FalseOr::Value(number) {
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

                    if self.time_bins.len() > 1 &&
                        let EventData::Neutron { t, .. } = &mut event.data
                    {
                        // find the correct bin for this relative time
                        // this can never overflow, since the final bin is guaranteed
                        // to be the EventTime::MAX
                        while event.rel_time >= self.time_bins[self.cur_bin] {
                             self.cur_bin += 1;
                        }
                        *t = self.cur_bin as u32;
                    }
                }
            }
        }
        events
    }
}

pub struct HistoStd {
    // Configuration
    /// Whether to use the gate signal to filter events. If false, gate signals
    /// are ignored.
    use_gate: bool,
    // Run-time state
    gate_up: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct HistoConfig {
    pub use_gate: Option<bool>,
}

impl Recipe for HistoStd {
    fn from_config(config: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        let config: HistoConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        Ok(HistoStd {
            use_gate: config.use_gate.unwrap_or(false),
            gate_up: false,
        })
    }

    fn update_config(&mut self, config: toml::Table) -> UResult<()> {
        let config: HistoConfig = config.try_into()
            .context("parsing config for tof_std recipe")?;
        if let Some(use_gate) = config.use_gate {
            self.use_gate = use_gate;
        }
        Ok(())
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
                }
            }
        }
        events
    }
}
