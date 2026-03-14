// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::time::Instant;
use anyhow::Context;
use crate::{lprintln, ltrace};
use crate::channel::{Receiver, Sender};
use crate::command::CommandReply;
use crate::error::{UResult};
use crate::event::{EventData, EventTime};
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;
use crate::shm::ShmBox;

pub struct PostProcessor {
    recipes: BTreeMap<String, Box<dyn Recipe>>,
    input: Receiver<PipeItem>,
    output: Sender<PipeItem>,
    shm: ShmBox,
}

impl PostProcessor {
    pub fn new(
        recipes: BTreeMap<String, Box<dyn Recipe>>,
        input: Receiver<PipeItem>,
        output: Sender<PipeItem>,
        data: ShmBox,
    ) -> Self {
        Self {
            recipes,
            input,
            output,
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
        let mut started = None;
        let mut debug_at = 0;
        let mut last_ts = EventTime::zero();
        let mut ev_count: usize = 0;
        let mut out_of_order = 0;

        // use the default recipe at first
        let recipe_names = self.recipes.keys().cloned().collect::<Vec<_>>();
        let mut recipe = self.recipes.get_mut("default").expect("default recipe").as_mut();

        self.shm.set_initialized();

        while let Ok(mut item) = self.input.recv() {
            match item {
                PipeItem::Events(evs) => {
                    ev_count += evs.len();
                    let evs = recipe.process(evs);
                    ltrace!("Postprocessed events: {:?}", evs);
                    if ev_count > debug_at {
                        lprintln!(DEBUG, "Received {} events", ev_count);
                        debug_at += 1000000;
                    }
                    for ev in &evs {
                        let ev_ts = ev.time;
                        if ev_ts < last_ts {
                            out_of_order += 1;
                        }
                        last_ts = ev_ts;

                        if let EventData::Neutron { x, y, t } = ev.data {
                            self.shm.add_histo(x, y, t);
                        };
                    }
                    item = PipeItem::Events(evs);
                }
                PipeItem::StartOfRun(ref run_id) => {
                    lprintln!(INFO, "Starting run {}", run_id);
                    self.shm.set_run_id(run_id);
                    last_ts = EventTime::zero();
                    ev_count = 0;
                    out_of_order = 0;
                    debug_at = 0;
                    started = Some(Instant::now());
                }
                PipeItem::EndOfRun => {
                    if let Some(ts) = started {
                        lprintln!(INFO, "Run finished: {} events in {:.3} s, {} out of order",
                                  ev_count, ts.elapsed().as_secs_f32(), out_of_order);
                        started = None;
                    }
                }
                PipeItem::Clear => {
                    lprintln!(INFO, "Clearing histogram");
                    self.shm.clear_histo();
                }

                // Meta items, not sent on to output
                PipeItem::ModuleState(ref module, ref state) => {
                    self.shm.set_state(*module, *state);
                    continue;
                }
                PipeItem::SetMode(name, params, send) => {
                    lprintln!(INFO, "Using postproc recipe {} with {:?}", name, params);
                    if !recipe_names.contains(&name) {
                        send.send(CommandReply::new_error(
                            None, format!("Recipe {} not found", name)))
                            .expect("param reply receiver died");
                        continue;
                    }
                    recipe = self.recipes.get_mut(&name).expect("checked above").as_mut();
                    send.send(match recipe.update_config(params) {
                        Ok(_) => CommandReply::Ok,
                        Err(e) => CommandReply::new_error(
                            None, format!("Failed to update recipe config: {}", e)),
                    }).expect("param reply receiver died");
                    continue;
                }
                PipeItem::GetModes(send) => {
                    send.send(CommandReply::Data { module: None, value: recipe_names.clone().into() })
                        .expect("param reply receiver died");
                    continue;
                }
            }
            self.output.send(item).expect("output sender closed");
        }
    }
}
