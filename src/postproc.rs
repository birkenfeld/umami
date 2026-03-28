// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use anyhow::Context;
use crate::{lprintln, ltrace};
use crate::channel::{Receiver, Sender};
use crate::command::CommandReply;
use crate::error::{UResult};
use crate::event::{EventData, ModuleId};
use crate::input::InputState;
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;
use crate::shm::ShmBox;

pub struct PostProcessor {
    recipe_names: BTreeMap<String, usize>,
    recipes: Vec<Box<dyn Recipe>>,
    default_recipe: String,
    input: Receiver<PipeItem>,
    output: Sender<PipeItem>,
    input_state: BTreeMap<ModuleId, InputState>,
    shm: ShmBox,
}

impl PostProcessor {
    pub fn new(
        recipe_map: BTreeMap<String, Box<dyn Recipe>>,
        default_recipe: String,
        input: Receiver<PipeItem>,
        output: Sender<PipeItem>,
        data: ShmBox,
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
        let mut cur_recipe = String::from("default");
        let mut recipe = *self.recipe_names.get(&self.default_recipe)
                                           .expect("default recipe exists");

        self.shm.set_initialized();

        while let Ok(mut item) = self.input.recv() {
            match item {
                PipeItem::Events(evs) => {
                    let evs = self.recipes[recipe].process(evs);
                    ltrace!("Processed events: {:?}", evs);
                    for ev in &evs {
                        if let EventData::Neutron { x, y, t } = ev.data {
                            self.shm.add_histo(x, y, t);
                        };
                    }
                    item = PipeItem::Events(evs);
                }
                PipeItem::StartOfRun(ref run_id) => {
                    current_run = run_id.clone();
                    lprintln!(INFO, "Run {current_run:?} started");
                    self.shm.set_run_id(run_id);
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
                    for (name, &index) in &self.recipe_names {
                        match self.recipes[index].get_params() {
                            Ok(params) => {
                                send.send((name.into(), params)).expect("param reply receiver died");
                            }
                            Err(e) => {
                                lprintln!(ERROR, "Error getting parameters for recipe {}: {e:#}", name);
                            }
                        }
                    }
                }
                PipeItem::SetParams(ref mut param_map, ref send) => {
                    for (name, &index) in &self.recipe_names {
                        if let Some(params) = param_map.remove(name) {
                            if let Err(e) = self.recipes[index].update_params(name, params) {
                                lprintln!(ERROR, "Error setting parameters for recipe {}: {e:#}", name);
                                send.send(CommandReply::new_error(
                                    None, format!("Failed to set parameters for recipe {}: {e:#}", name)))
                                    .expect("param reply receiver died");
                            } else {
                                send.send(CommandReply::Ok).expect("param reply receiver died");
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
                    let modes = self.recipe_names.keys().cloned().collect::<Vec<_>>();
                    send.send(CommandReply::Data { value: modes.into() })
                        .expect("param reply receiver died");
                    continue;
                }
                PipeItem::SetMode(name, send) => {
                    lprintln!(INFO, "Using processing recipe {name}");
                    if !self.recipe_names.contains_key(&name) {
                        send.send(CommandReply::new_error(
                            None, format!("Recipe {} not found", name)))
                            .expect("param reply receiver died");
                        continue;
                    }
                    recipe = *self.recipe_names.get(&name).expect("checked above");
                    cur_recipe = name;
                    send.send(CommandReply::Ok).expect("param reply receiver died");
                    continue;
                }
                PipeItem::GetState(send) => {
                    let inputs = self.input_state.iter().map(|(mid, state)| {
                        let state = serde_json::to_value(state).expect("ok");
                        (mid.0.to_string(), state)
                    }).collect::<serde_json::Map<_, _>>();
                    let mut map = serde_json::Map::new();
                    map.insert("inputs".into(), inputs.into());
                    map.insert("mode".into(), cur_recipe.as_str().into());
                    // TODO mode parameters
                    send.send(CommandReply::Data { value: map.into() })
                        .expect("param reply receiver died");
                    continue;
                }
                PipeItem::SaveHisto(filename, max_nt, send) => {
                    send.send(match self.shm.save_to_file(&filename, max_nt) {
                        Ok(_) => CommandReply::Ok,
                        Err(e) => CommandReply::new_error(
                            None, format!("Failed to save histogram: {e:#}")
                        )
                    }).expect("param reply receiver died");
                    continue;
                }
            }
            self.output.send(item).expect("output sender closed");
        }
    }
}
