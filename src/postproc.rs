// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use itertools::Itertools;
use crate::{lprintln, ltrace};
use crate::channel::{Receiver, Sender};
use crate::command::{CommandReply, ModuleId};
use crate::error::{UResult};
use crate::event::EventType;
use crate::input::InputState;
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;
use crate::shm::ShmBox;

pub struct PostProcessor {
    recipe_names: BTreeMap<ModuleId, usize>,
    recipes: Vec<Box<dyn Recipe>>,
    default_recipe: ModuleId,
    input: Receiver<PipeItem>,
    output: Sender<PipeItem>,
    input_state: BTreeMap<ModuleId, InputState>,
    shm: ShmBox,
    instance_name: Option<String>,
}

impl PostProcessor {
    pub fn new(
        recipe_map: BTreeMap<ModuleId, Box<dyn Recipe>>,
        default_recipe: ModuleId,
        input: Receiver<PipeItem>,
        output: Sender<PipeItem>,
        data: ShmBox,
        instance_name: Option<String>,
    ) -> Self {
        let mut recipe_names = BTreeMap::new();
        let mut recipes = vec![];
        for (name, recipe) in recipe_map {
            recipe_names.insert(name, recipes.len());
            recipes.push(recipe);
        }

        Self {
            recipe_names,
            recipes,
            default_recipe,
            input,
            output,
            input_state: BTreeMap::new(),
            shm: data,
            instance_name,
        }
    }

    pub fn start(self) -> UResult<()> {
        std::thread::Builder::new()
            .name("Postprocessor".into())
            .spawn(move || self.main())
            .context("Spawning postprocessor thread")?;
        Ok(())
    }

    pub fn main(mut self) {
        let mut current_run = String::new();

        // use the default recipe at first
        let mut cur_recipe = ModuleId::new("default".into());
        let mut recipe = *self.recipe_names.get(&self.default_recipe)
                                           .expect("default recipe exists");

        self.shm.set_initialized();

        while let Ok(mut item) = self.input.recv() {
            match item {
                PipeItem::Events(evs) => {
                    let evs = self.recipes[recipe].process(evs);
                    ltrace!("Processed events: {:?}", evs);
                    for ev in &evs {
                        if let EventType::Neutron = ev.evtype {
                            self.shm.add_histo(ev.histo);
                        }
                    }
                    item = PipeItem::Events(evs);
                }
                PipeItem::StartOfRun(ref run_id) => {
                    current_run = run_id.clone();
                    lprintln!(INFO, "Run {current_run:?} started");
                    self.shm.set_run_id(run_id);
                    for r in &mut self.recipes {
                        r.start_of_run();
                    }
                }
                PipeItem::EndOfRun => {
                    lprintln!(INFO, "Run {current_run:?} finished");
                }
                PipeItem::Clear => {
                    lprintln!(INFO, "Clearing histogram");
                    self.shm.clear_histo();
                }

                // Meta items, sent on to outputs
                PipeItem::GetParams(ref send) => {
                    for (&name, &index) in &self.recipe_names {
                        match self.recipes[index].get_params() {
                            Ok(params) => {
                                let _ = send.send((name, params));
                            }
                            Err(e) => {
                                lprintln!(ERROR, "Error getting parameters for recipe {}: {e:#}",
                                          name);
                            }
                        }
                    }
                }
                PipeItem::SetParams(ref mut param_map, ref send) => {
                    for (&name, &index) in &self.recipe_names {
                        if let Some(params) = param_map.remove(&name) {
                            if let Err(e) = self.recipes[index].update_params(name, params) {
                                lprintln!(ERROR, "Error setting parameters for recipe {}: {e:#}",
                                          name);
                                let _ = send.send(CommandReply::new_mod_error(
                                    name, format!("Failed to set parameters: {e:#}")
                                ));
                            } else {
                                let _ = send.send(CommandReply::Ok);
                            }
                        }
                    }
                }

                // Meta items, not sent on to outputs
                PipeItem::InputState(module, state) => {
                    self.input_state.insert(module, state);
                    continue;
                }
                PipeItem::GetModes(send) => {
                    let modes = self.recipe_names.keys().map(|s| s.to_string()).collect_vec();
                    let _ = send.send(CommandReply::Data { value: modes.into() });
                    continue;
                }
                PipeItem::SetMode(name, send) => {
                    if !self.recipe_names.contains_key(&name) {
                        let _ = send.send(CommandReply::new_error(
                            format!("Recipe {name} not found")));
                        continue;
                    }
                    lprintln!(INFO, "Using processing recipe {name}");
                    recipe = *self.recipe_names.get(&name).expect("checked above");
                    cur_recipe = name;
                    let _ = send.send(CommandReply::Ok);
                    continue;
                }
                PipeItem::GetState(send) => {
                    let inputs = self.input_state.iter().map(|(mid, state)| {
                        let state = serde_json::to_value(state).expect("ok");
                        (mid.to_string(), state)
                    }).collect::<serde_json::Map<_, _>>();
                    let mut map = serde_json::Map::new();
                    map.insert("inputs".into(), inputs.into());
                    map.insert("mode".into(), cur_recipe.as_str().into());
                    map.insert("name".into(), serde_json::json!(self.instance_name));
                    // TODO mode parameters
                    let _ = send.send(CommandReply::Data { value: map.into() });
                    continue;
                }
                PipeItem::SaveHisto(filename, max_nt, send) => {
                    let _ = send.send(match self.shm.save_to_file(&filename, max_nt) {
                        Ok(()) => CommandReply::Ok,
                        Err(e) => {
                            lprintln!(ERROR, "Error saving histogram to file \
                                              {filename}: {e:#}");
                            CommandReply::new_error(
                                format!("Failed to save histogram: {e:#}")
                            )
                        }
                    });
                    continue;
                }
            }
            self.output.send(item).expect("output sender closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel;
    use crate::config::{HistoConfig, RecipeConfig};
    use crate::event::test_utils;
    use crate::params::ParamMap;
    use crate::recipe;
    use crate::shm::ShmGuard;

    fn build_recipes(specs: &[(&str, &str)]) -> BTreeMap<ModuleId, Box<dyn Recipe>> {
        let mut configs = BTreeMap::new();
        for (name, typ) in specs {
            configs.insert((*name).to_string(),
                           RecipeConfig { r#type: (*typ).to_string(), config: toml::Table::new() });
        }
        specs.iter().map(|(name, _)| {
            (ModuleId::new((*name).to_string()), recipe::from_config(&configs, name).unwrap())
        }).collect()
    }

    /// Spins up a real PostProcessor thread wired to fresh shared memory and
    /// returns the channels to drive it plus a guard owning its shm segment.
    fn make_postproc(specs: &[(&str, &str)], default: &str)
        -> (channel::Sender<PipeItem>, channel::Receiver<PipeItem>, ShmGuard)
    {
        let shm_guard = ShmGuard::unique();
        let histo_config = HistoConfig { nx: 4, ny: 4, max_nt: 1, max_ni: 0 };
        let shm = crate::shm::ShmInterface::create(shm_guard.name(), &histo_config).unwrap();
        let (input_send, input_recv) = channel::bounded(16);
        let (output_send, output_recv) = channel::bounded(16);
        let postproc = PostProcessor::new(
            build_recipes(specs), ModuleId::new(default.to_string()), input_recv, output_send, shm,
            None,
        );
        postproc.start().unwrap();
        (input_send, output_recv, shm_guard)
    }

    /// Sends a `GetState` request and blocks for its reply, purely to use as a
    /// synchronization barrier: since the channel preserves order and the
    /// postprocessor handles items strictly sequentially, any item sent before
    /// this call is guaranteed fully processed once this returns.
    fn sync_barrier(input: &channel::Sender<PipeItem>) {
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::GetState(send)).unwrap();
        recv.recv().unwrap();
    }

    #[test]
    fn test_postproc_mode_switching_and_state() {
        let (input, _output, _shm) =
            make_postproc(&[("std", "histo_std"), ("tof", "histo_tof")], "std");

        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::GetModes(send)).unwrap();
        let modes = match recv.recv().unwrap() {
            CommandReply::Data { value } => value.as_array().unwrap().iter()
                .map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>(),
            other => panic!("unexpected reply: {other:?}"),
        };
        assert!(modes.contains(&"std".to_string()));
        assert!(modes.contains(&"tof".to_string()));

        // switching to an unknown mode fails, and doesn't change the current one
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SetMode(ModuleId::new("bogus".into()), send)).unwrap();
        assert!(recv.recv().unwrap().is_error());

        // switching to a known mode succeeds and is reflected in GetState
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SetMode(ModuleId::new("tof".into()), send)).unwrap();
        assert!(matches!(recv.recv().unwrap(), CommandReply::Ok));

        // input state is tracked and reported too
        input.send(PipeItem::InputState(ModuleId::new("in1".into()), InputState::Running)).unwrap();

        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::GetState(send)).unwrap();
        match recv.recv().unwrap() {
            CommandReply::Data { value } => {
                assert_eq!(value["mode"], "tof");
                assert_eq!(value["inputs"]["in1"], "running");
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn test_postproc_get_state_reports_instance_name() {
        let shm_guard = ShmGuard::unique();
        let histo_config = HistoConfig { nx: 4, ny: 4, max_nt: 1, max_ni: 0 };
        let shm = crate::shm::ShmInterface::create(shm_guard.name(), &histo_config).unwrap();
        let (input_send, input_recv) = channel::bounded(16);
        let (output_send, _output_recv) = channel::bounded(16);
        let postproc = PostProcessor::new(
            build_recipes(&[("std", "histo_std")]), ModuleId::new("std".into()),
            input_recv, output_send, shm, Some("My Detector".into()),
        );
        postproc.start().unwrap();

        let (send, recv) = channel::bounded(1);
        input_send.send(PipeItem::GetState(send)).unwrap();
        match recv.recv().unwrap() {
            CommandReply::Data { value } => assert_eq!(value["name"], "My Detector"),
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn test_postproc_switching_to_tof_mode_mid_run_does_not_use_stale_t0() {
        // a recipe that just became active must not use T0 state from before
        let (input, output, _shm) =
            make_postproc(&[("std", "histo_std"), ("tof", "histo_tof")], "std");
        let timeout = std::time::Duration::from_secs(5);

        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SetMode(ModuleId::new("tof".into()), send)).unwrap();
        assert!(matches!(recv.recv().unwrap(), CommandReply::Ok));

        input.send(PipeItem::Events(vec![test_utils::neutron(999_999_999_999, 1)])).unwrap();
        match output.recv_timeout(timeout).expect("forwarded item") {
            PipeItem::Events(evs) => assert_eq!(evs[0].evtype, EventType::Void),
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn test_postproc_params_get_and_set() {
        let (input, output, _shm) = make_postproc(&[("std", "histo_std")], "std");
        let timeout = std::time::Duration::from_secs(5);

        // GetParams/SetParams (unlike the other meta items) are forwarded on to the
        // output chain afterwards, carrying the reply Sender along with them; in real
        // use the last output in the chain drops it once there's nowhere further to
        // send. Here we simulate that by draining and dropping the forwarded item
        // ourselves -- otherwise the reply channel never closes and the blocking
        // `into_iter()` below would hang forever.
        let (send, recv) = channel::bounded(2);
        input.send(PipeItem::GetParams(send)).unwrap();
        output.recv_timeout(timeout).expect("forwarded item");
        let params: Vec<_> = recv.into_iter().collect();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, ModuleId::new("std".into()));
        assert_eq!(params[0].1["bin_x"]["value"], 1);

        // a valid update is applied and shows up in a later GetParams
        let mut new_params = ParamMap::new();
        new_params.insert("bin_x".into(), serde_json::json!(4));
        let mut set_map = BTreeMap::new();
        set_map.insert(ModuleId::new("std".into()), new_params);
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SetParams(set_map, send)).unwrap();
        output.recv_timeout(timeout).expect("forwarded item");
        assert!(matches!(recv.recv().unwrap(), CommandReply::Ok));

        let (send, recv) = channel::bounded(2);
        input.send(PipeItem::GetParams(send)).unwrap();
        output.recv_timeout(timeout).expect("forwarded item");
        let params: Vec<_> = recv.into_iter().collect();
        assert_eq!(params[0].1["bin_x"]["value"], 4);

        // an update with a value of the wrong type fails
        let mut bad_params = ParamMap::new();
        bad_params.insert("bin_x".into(), serde_json::json!("not a number"));
        let mut set_map = BTreeMap::new();
        set_map.insert(ModuleId::new("std".into()), bad_params);
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SetParams(set_map, send)).unwrap();
        output.recv_timeout(timeout).expect("forwarded item");
        assert!(recv.recv().unwrap().is_error());
    }

    #[test]
    fn test_postproc_histogram_accumulation_clear_and_save() {
        let (input, _output, shm) = make_postproc(&[("std", "histo_std")], "std");

        let events = vec![
            test_utils::neutron_xy(100, 0, 1, 2),
            test_utils::neutron_xy(200, 0, 1, 2),
            test_utils::edge(300, 0, true), // not a neutron, shouldn't be histogrammed
        ];
        input.send(PipeItem::Events(events)).unwrap();
        sync_barrier(&input);

        let shm_read = crate::shm::ShmInterface::open(shm.name()).unwrap();
        let histo = shm_read.histo_data();
        assert_eq!(histo.iter().sum::<u32>(), 2);
        assert_eq!(histo[2 * 4 + 1], 2); // offset for (x=1, y=2, t=0)

        input.send(PipeItem::Clear).unwrap();
        sync_barrier(&input);
        assert!(shm_read.histo_data().iter().all(|&v| v == 0));

        input.send(PipeItem::Events(vec![test_utils::neutron_xy(100, 0, 0, 0)])).unwrap();
        let path = format!("/tmp/umami_postproc_test_histo_{}", std::process::id());
        let (send, recv) = channel::bounded(1);
        input.send(PipeItem::SaveHisto(path.clone(), 1, send)).unwrap();
        assert!(matches!(recv.recv().unwrap(), CommandReply::Ok));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains('1'));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_postproc_meta_items_not_forwarded() {
        let (input, output, _shm) = make_postproc(&[("std", "histo_std")], "std");

        // meta items are consumed by the postprocessor and never reach the output
        let (send, _recv) = channel::bounded(1);
        input.send(PipeItem::GetModes(send)).unwrap();
        let (send, _recv) = channel::bounded(1);
        input.send(PipeItem::GetState(send)).unwrap();
        input.send(PipeItem::InputState(ModuleId::new("in1".into()), InputState::Running)).unwrap();

        // regular items pass through to the output once processed
        input.send(PipeItem::StartOfRun("run1".into())).unwrap();
        input.send(PipeItem::Events(vec![test_utils::neutron(100, 0)])).unwrap();
        input.send(PipeItem::EndOfRun).unwrap();

        let forwarded: Vec<_> = (0..3).map(|_| output.recv_timeout(std::time::Duration::from_secs(5))
                                                     .expect("expected forwarded item")).collect();
        assert!(matches!(forwarded[0], PipeItem::StartOfRun(_)));
        assert!(matches!(forwarded[1], PipeItem::Events(_)));
        assert!(matches!(forwarded[2], PipeItem::EndOfRun));
        // nothing else should have been forwarded
        assert!(output.try_recv().is_err());
    }
}
