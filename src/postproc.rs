// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use shmem_bind::ShmemBox;
use crate::{lprintln, ltrace};
use crate::channel::{Sender, Receiver};
use crate::config::HistoConfig;
use crate::error::{UResult};
use crate::event::{EventData, EventTime};
use crate::histo::Histogram;
use crate::interface::ShmInterface;
use crate::pipeline::PipeItem;
use crate::recipe::Recipe;

pub struct PostProcessor {
    recipe: Box<dyn Recipe>,
    input: Receiver<PipeItem>,
    output: Sender<PipeItem>,
    histo: Histogram,
    shm_data: ShmemBox<ShmInterface>,
}

impl PostProcessor {
    pub fn new(recipe: Box<dyn Recipe>,
               input: Receiver<PipeItem>,
               output: Sender<PipeItem>,
               data: ShmemBox<ShmInterface>,
               config: HistoConfig,
    ) -> Self {
        let histo = Histogram::new(config.nx, config.ny);
        Self { recipe, input, output, histo, shm_data: data }
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
                    self.histo.clear();
                }
                PipeItem::StartOfRun(run_id) => {
                    lprintln!(INFO, "Starting run {}", run_id);
                }
                PipeItem::EndOfRun => {
                    let stop = jiff::Timestamp::now();
                    println!("Final count: {} events in {} secs, {} out of order", i, stop - start, ooo);
                    i = 0;
                    ooo = 0;
                    self.histo.plot();
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
                            self.histo.add(x as usize, y as usize);
                        }
                    }
                }
                PipeItem::TofParams { .. } => {
                    // TODO self.recipe.set_tof_params(nt, dt, t0);
                }
                PipeItem::State(_) => {
                    // TODO
                }
            }
            // self.output.send(()).unwrap();
        }
    }
}
