// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::{Event, EventType, EventFlags, EventTime};
use crate::params::HasParams;
use crate::recipe::Recipe;

#[derive(HasParams)]
#[params(kind = "recipe", type = "histo_tof")]
pub struct Tof {
    // Configuration
    /// Binning in spatial directions.
    #[param(help="Binning in x direction")]
    bin_x: u16,
    #[param(help="Binning in y direction")]
    bin_y: u16,
    /// Whether to use the gate signal to filter events. If false, gate signals
    /// are ignored.
    #[param(help="Whether to use gate signal")]
    use_gate: bool,
    /// If set, this is the value of the AuxSignal that is used as T0. If None,
    /// T0 is given by Tzero events.
    #[param(help="Aux signal number to use as T0, or false to use explicit T0 events",
            datatype="null or integer")]
    aux_mode: Option<u8>,
    /// Contains end of time bins since T0.
    #[param(help="Time binning end times (first bin always starts at offset 0)",
            datatype="array of integers (nanoseconds)", has_setter=true)]
    time_bins: Vec<EventTime>,
    // Run-time state
    gate_up: bool,
    // TODO: consider frame overlap
    last_t0: EventTime,
    cur_bin: usize,
    // whether last_t0 reflects a Tzero/AuxSignal seen since `start_of_run`
    t0_known: bool,
}

impl Tof {
    /// The last bin edge is always EventTime::MAX, so a relative time past
    /// the last configured edge still indexes a valid bin.
    fn normalize_time_bins(mut bins: Vec<EventTime>) -> Vec<EventTime> {
        if bins.last() != Some(&EventTime::MAX) {
            bins.push(EventTime::MAX);
        }
        bins
    }

    fn set_time_bins(&mut self, value: Vec<EventTime>) -> UResult<()> {
        self.time_bins = Self::normalize_time_bins(value);
        self.cur_bin = 0;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TofConfig {
    pub bin_x: Option<u16>,
    pub bin_y: Option<u16>,
    pub use_gate: Option<bool>,
    pub aux_mode: Option<u8>,
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
            time_bins: Self::normalize_time_bins(config.time_bins.unwrap_or_default()),
            gate_up: false,
            last_t0: EventTime::zero(),
            cur_bin: 0,
            t0_known: false,
        })
    }

    fn start_of_run(&mut self) {
        self.last_t0 = EventTime::zero();
        self.cur_bin = 0;
        self.t0_known = false;
        self.gate_up = false;
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Tzero => {
                    if self.aux_mode.is_none() {
                        self.last_t0 = event.time;
                        self.cur_bin = 0;
                        self.t0_known = true;
                    }
                }
                EventType::AuxSignal => {
                    if self.aux_mode == Some(event.index as _) {
                        self.last_t0 = event.time;
                        self.cur_bin = 0;
                        self.t0_known = true;
                    }
                }
                EventType::Gate => self.gate_up = event.index != 0,
                _ => {
                    if self.use_gate && !self.gate_up {
                        event.evtype = EventType::Void;
                        continue;
                    }
                    if !self.t0_known {
                        event.evtype = EventType::Void;
                        continue;
                    }

                    if !event.flags.contains(EventFlags::HasRelTime) {
                        event.rel_time = event.time - self.last_t0;
                        event.flags.set(EventFlags::HasRelTime);
                    }

                    if let EventType::Neutron = event.evtype {
                        if self.time_bins.len() > 1 {
                            // find the correct bin for this relative time
                            // this can never overflow, since the final bin is guaranteed
                            // to be the EventTime::MAX
                            while event.rel_time >= self.time_bins[self.cur_bin] {
                                self.cur_bin += 1;
                            }
                            event.histo.t = self.cur_bin as u16;
                        }
                        event.histo.x /= self.bin_x;
                        event.histo.y /= self.bin_y;
                    }
                }
            }
        }
        events
    }
}


#[derive(HasParams)]
#[params(kind = "recipe", type = "histo_std")]
pub struct Std {
    // Configuration
    /// Binning in spatial directions.
    #[param(help="Binning in x direction")]
    bin_x: u16,
    #[param(help="Binning in y direction")]
    bin_y: u16,
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
    pub bin_x: Option<u16>,
    pub bin_y: Option<u16>,
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

    fn start_of_run(&mut self) {
        self.gate_up = false;
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            match event.evtype {
                EventType::Gate => self.gate_up = event.index != 0,
                _ => {
                    if self.use_gate && !self.gate_up {
                        event.evtype = EventType::Void;
                        continue;
                    }

                    if let EventType::Neutron = event.evtype {
                        event.histo.x /= self.bin_x;
                        event.histo.y /= self.bin_y;
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ModuleId;
    use crate::event::test_utils;
    use crate::event::EventTime;
    use crate::params::ParamMap;

    fn empty_recipes() -> BTreeMap<String, RecipeConfig> {
        BTreeMap::new()
    }

    #[test]
    fn test_std_passthrough_default() {
        let mut recipe = Std::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let events = vec![
            test_utils::neutron(100, 1),
            test_utils::neutron(200, 2),
        ];
        let out = recipe.process(events);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].channel.0, 1);
        assert_eq!(out[1].channel.0, 2);
    }

    #[test]
    fn test_std_binning() {
        let mut cfg = toml::Table::new();
        cfg.insert("bin_x".into(), toml::Value::Integer(2));
        cfg.insert("bin_y".into(), toml::Value::Integer(3));
        let mut recipe = Std::from_config(cfg, &empty_recipes()).unwrap();

        let mut ev = test_utils::neutron(100, 1);
        ev.histo.x = 7;
        ev.histo.y = 8;
        let out = recipe.process(vec![ev]);
        assert_eq!(out[0].histo.x, 3);  // 7 / 2
        assert_eq!(out[0].histo.y, 2);  // 8 / 3
    }

    #[test]
    fn test_std_gate_filtering_off() {
        let mut cfg = toml::Table::new();
        cfg.insert("use_gate".into(), toml::Value::Boolean(false));
        let mut recipe = Std::from_config(cfg, &empty_recipes()).unwrap();

        let events = vec![
            test_utils::gate(50, true),
            test_utils::neutron(100, 1),
        ];
        let out = recipe.process(events);
        // gate signal consumed, neutron passes through unchanged
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].evtype, EventType::Gate);
        assert_eq!(out[1].evtype, EventType::Neutron);
    }

    #[test]
    fn test_std_gate_filtering_on_before_gate() {
        let mut cfg = toml::Table::new();
        cfg.insert("use_gate".into(), toml::Value::Boolean(true));
        let mut recipe = Std::from_config(cfg, &empty_recipes()).unwrap();

        // neutron before any gate up → voided
        let events = vec![test_utils::neutron(100, 1)];
        let out = recipe.process(events);
        assert_eq!(out[0].evtype, EventType::Void);
    }

    #[test]
    fn test_std_gate_filtering_on_after_gate_up() {
        let mut cfg = toml::Table::new();
        cfg.insert("use_gate".into(), toml::Value::Boolean(true));
        let mut recipe = Std::from_config(cfg, &empty_recipes()).unwrap();

        let events = vec![
            test_utils::gate(50, true),
            test_utils::neutron(100, 1),
            test_utils::gate(150, false),
            test_utils::neutron(200, 2),
        ];
        let out = recipe.process(events);
        assert_eq!(out[0].evtype, EventType::Gate);
        assert_eq!(out[1].evtype, EventType::Neutron);  // passes
        assert_eq!(out[2].evtype, EventType::Gate);
        assert_eq!(out[3].evtype, EventType::Void);  // voided
    }

    #[test]
    fn test_tof_basic() {
        let mut recipe = Tof::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let events = vec![
            test_utils::tzero(0),
            test_utils::neutron(500, 1),
        ];
        let out = recipe.process(events);
        assert_eq!(out[0].evtype, EventType::Tzero);
        assert_eq!(out[1].evtype, EventType::Neutron);
        // rel_time = 500 - 0 = 500
        assert_eq!(out[1].rel_time, EventTime(500));
        assert!(out[1].flags.contains(EventFlags::HasRelTime));
    }

    #[test]
    fn test_tof_gate_filtering() {
        let mut cfg = toml::Table::new();
        cfg.insert("use_gate".into(), toml::Value::Boolean(true));
        let mut recipe = Tof::from_config(cfg, &empty_recipes()).unwrap();

        let events = vec![
            test_utils::tzero(0),
            test_utils::gate(100, true),
            test_utils::neutron(200, 1),
            test_utils::gate(300, false),
            test_utils::neutron(400, 2),
        ];
        let out = recipe.process(events);
        assert_eq!(out[1].evtype, EventType::Gate);
        assert_eq!(out[2].evtype, EventType::Neutron);  // passes
        assert_eq!(out[3].evtype, EventType::Gate);
        assert_eq!(out[4].evtype, EventType::Void);  // voided
    }

    #[test]
    fn test_tof_aux_mode() {
        let mut cfg = toml::Table::new();
        cfg.insert("aux_mode".into(), toml::Value::Integer(2));
        let mut recipe = Tof::from_config(cfg, &empty_recipes()).unwrap();

        // wrong aux → ignored
        let events = vec![
            test_utils::aux(100, 1),
            test_utils::aux(200, 2),
            test_utils::neutron(300, 1),
        ];
        let out = recipe.process(events);
        // t0 set by aux(200, 2), rel_time = 300 - 200 = 100
        assert_eq!(out[2].rel_time, EventTime(100));
    }

    #[test]
    fn test_tof_neutron_before_any_tzero_is_voided() {
        let mut recipe = Tof::from_config(toml::Table::new(), &empty_recipes()).unwrap();
        let out = recipe.process(vec![test_utils::neutron(999_999_999_999, 1)]);
        assert_eq!(out[0].evtype, EventType::Void);
    }

    #[test]
    fn test_tof_start_forgets_stale_t0_until_a_fresh_one_arrives() {
        let mut recipe = Tof::from_config(toml::Table::new(), &empty_recipes()).unwrap();

        let out = recipe.process(vec![test_utils::tzero(1000),
                                       test_utils::neutron(1100, 1)]);
        assert_eq!(out[1].rel_time, EventTime(100));

        recipe.start_of_run();
        let out = recipe.process(vec![test_utils::neutron(999_999_999_999, 2)]);
        assert_eq!(out[0].evtype, EventType::Void);

        let out = recipe.process(vec![test_utils::tzero(2000),
                                       test_utils::neutron(2050, 3)]);
        assert_eq!(out[1].rel_time, EventTime(50));
    }

    #[test]
    fn test_tof_binning() {
        let mut cfg = toml::Table::new();
        cfg.insert("bin_x".into(), toml::Value::Integer(2));
        cfg.insert("bin_y".into(), toml::Value::Integer(4));
        let mut recipe = Tof::from_config(cfg, &empty_recipes()).unwrap();

        let mut ev = test_utils::neutron(100, 1);
        ev.histo.x = 10;
        ev.histo.y = 15;
        let out = recipe.process(vec![test_utils::tzero(0), ev]);
        assert_eq!(out[1].histo.x, 5);   // 10 / 2
        assert_eq!(out[1].histo.y, 3);   // 15 / 4
    }

    #[test]
    fn test_tof_time_bins() {
        let mut cfg = toml::Table::new();
        let bins = toml::Value::Array(vec![
            toml::Value::Integer(1000),
            toml::Value::Integer(2000),
            toml::Value::Integer(i64::MAX),
        ]);
        cfg.insert("time_bins".into(), bins);
        let mut recipe = Tof::from_config(cfg, &empty_recipes()).unwrap();

        let events = vec![
            test_utils::tzero(0),
            test_utils::neutron(500, 1),   // bin 0 (500 < 1000)
            test_utils::neutron(1500, 2),  // bin 1 (1000 <= 1500 < 2000)
            test_utils::neutron(2500, 3),  // bin 2 (2000 <= 2500)
        ];
        let out = recipe.process(events);
        assert_eq!(out[1].histo.t, 0);
        assert_eq!(out[2].histo.t, 1);
        assert_eq!(out[3].histo.t, 2);
    }

    #[test]
    fn test_tof_time_bins_set_at_runtime_without_max_sentinel() {
        let mut recipe = Tof::from_config(toml::Table::new(), &empty_recipes()).unwrap();

        let mut params = ParamMap::new();
        params.insert("time_bins".into(), serde_json::json!([1000, 2000]));
        recipe.update_params(ModuleId::new("tof".into()), params).unwrap();

        let events = vec![
            test_utils::tzero(0),
            test_utils::neutron(500, 1),
            test_utils::neutron(1500, 2),
            test_utils::neutron(999_999, 3),
        ];
        let out = recipe.process(events);
        assert_eq!(out[1].histo.t, 0); // 0 to 1000 bin
        assert_eq!(out[2].histo.t, 1); // 1000 to 2000 bin
        assert_eq!(out[3].histo.t, 2); // last (implicit) 2000 to MAX bin
    }

    #[test]
    fn test_tof_existing_rel_time_preserved() {
        let mut recipe = Tof::from_config(toml::Table::new(), &empty_recipes()).unwrap();

        let mut ev = test_utils::neutron(100, 1);
        ev.rel_time = EventTime(999);
        ev.flags.set(EventFlags::HasRelTime);
        let out = recipe.process(vec![test_utils::tzero(0), ev]);
        // rel_time already set, should be preserved
        assert_eq!(out[1].rel_time, EventTime(999));
    }
}
