// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use crate::{lprintln, ltrace};
use crate::channel::{Receiver};
use crate::error::{UResult};
use crate::event::{EventData, EventTime};
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;
use crate::shm::ShmBox;

pub struct PostProcessor {
    recipe: Box<dyn Recipe>,
    input: Receiver<PipeItem>,
    // output: Sender<PipeItem>,
    shm: ShmBox,
}

impl PostProcessor {
    pub fn new(recipe: Box<dyn Recipe>,
               input: Receiver<PipeItem>,
               // output: Sender<PipeItem>,
               data: ShmBox,
    ) -> Self {
        Self { recipe, input, shm: data }
    }

    pub fn start(self) -> UResult<()> {
        std::thread::Builder::new()
            .name("Postprocessor".into())
            .spawn(move || self.main())
            .context("Spawning postprocessor thread")?;
        Ok(())
    }

    pub fn main(mut self) {
        let mut i: usize = 0;
        let mut limit = 0;
        let mut ts = EventTime::zero();
        let mut ooo = 0;
        let start = jiff::Timestamp::now();

        while let Ok(item) = self.input.recv() {
            match item {
                PipeItem::Clear => {
                    lprintln!(INFO, "Clearing histogram");
                    self.shm.clear_histo();
                }
                PipeItem::StartOfRun(run_id) => {
                    lprintln!(INFO, "Starting run {}", run_id);
                }
                PipeItem::EndOfRun => {
                    let stop = jiff::Timestamp::now();
                    println!("Final count: {} events in {} secs, {} out of order", i, stop - start, ooo);
                    i = 0;
                    ooo = 0;
                }
                PipeItem::Events(evs) => {
                    i += evs.len();
                    let evs = self.recipe.process(evs);
                    ltrace!("Postprocessed events: {:?}", evs);
                    if i > limit {
                        println!("Received {} events", i);
                        limit += 1000000;
                    }
                    for ev in evs {
                        let nts = ev.time;
                        if nts < ts {
                            ooo += 1;
                        }
                        ts = nts;

                        if let EventData::Neutron { x, y, .. } = ev.data {
                            self.shm.add_histo(x, y, 0);
                        }
                    }
                }
                PipeItem::TofParams { .. } => {
                    // TODO self.recipe.set_tof_params(nt, dt, t0);
                }
                PipeItem::State(state) => {
                    self.shm.set_state(state);
                }
            }
        }
    }
}
