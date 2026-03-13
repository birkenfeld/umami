// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

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
    recipe: Box<dyn Recipe>,
    input: Receiver<PipeItem>,
    output: Sender<PipeItem>,
    shm: ShmBox,
}

impl PostProcessor {
    pub fn new(
        recipe: Box<dyn Recipe>,
        input: Receiver<PipeItem>,
        output: Sender<PipeItem>,
        data: ShmBox,
    ) -> Self {
        Self {
            recipe,
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
        let mut limit = 0;
        let mut last_ts = EventTime::zero();
        let mut ev_count: usize = 0;
        let mut out_of_order = 0;
        let start = jiff::Timestamp::now();

        self.shm.set_initialized();

        while let Ok(mut item) = self.input.recv() {
            match item {
                PipeItem::Clear => {
                    lprintln!(INFO, "Clearing histogram");
                    self.shm.clear_histo();
                }
                PipeItem::StartOfRun(ref run_id) => {
                    lprintln!(INFO, "Starting run {}", run_id);
                    self.shm.set_run_id(run_id);
                    last_ts = EventTime::zero();
                    ev_count = 0;
                    out_of_order = 0;
                    limit = 0;
                }
                PipeItem::EndOfRun => {
                    let stop = jiff::Timestamp::now();
                    println!("Final count: {} events in {} secs, {} out of order",
                             ev_count, stop - start, out_of_order);
                }
                PipeItem::Events(evs) => {
                    ev_count += evs.len();
                    let evs = self.recipe.process(evs);
                    ltrace!("Postprocessed events: {:?}", evs);
                    if ev_count > limit {
                        println!("Received {} events", ev_count);
                        limit += 1000000;
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
                PipeItem::Params(params, send) => {
                    lprintln!(INFO, "Updating postproc recipe with {:?}", params);
                    send.send(match self.recipe.update_config(params) {
                        Ok(_) => CommandReply::Ok,
                        Err(e) => CommandReply::new_error(
                            None, format!("Failed to update recipe config: {}", e)),
                    }).expect("param reply receiver died");
                    continue;
                }
                PipeItem::State(ref module, ref state) => {
                    self.shm.set_state(*module, *state);
                }
            }
            self.output.send(item).expect("output sender closed");
        }
    }
}
