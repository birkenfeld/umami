// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use crate::{lprintln, ltrace};
use crate::channel::{Receiver, Sender};
use crate::config::HistoConfig;
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
    nt: usize,
    dt: EventTime,
    t0: EventTime,
}

impl PostProcessor {
    pub fn new(
        recipe: Box<dyn Recipe>,
        input: Receiver<PipeItem>,
        output: Sender<PipeItem>,
        data: ShmBox,
        config: &HistoConfig,
    ) -> Self {
        Self {
            recipe,
            input,
            output,
            shm: data,
            nt: 0,
            dt: EventTime::from_floating_sec(config.default_tbin),
            t0: EventTime::from_floating_sec(config.default_tdelay),
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
                    let mut evs = self.recipe.process(evs);
                    ltrace!("Postprocessed events: {:?}", evs);
                    if ev_count > limit {
                        println!("Received {} events", ev_count);
                        limit += 1000000;
                    }
                    for ev in &mut evs {
                        let ev_ts = ev.time;
                        if ev_ts < last_ts {
                            out_of_order += 1;
                        }
                        last_ts = ev_ts;

                        if let EventData::Neutron { x, y, ref mut t } = ev.data {
                            if self.nt == 0 {
                                self.shm.add_histo(x, y, 0);
                            } else {
                                let tbin = ev_ts.time_bin(self.dt, self.t0);
                                if tbin < self.nt as u32 {
                                    self.shm.add_histo(x, y, tbin);
                                    *t = tbin;
                                }
                            };
                        };
                    }
                    item = PipeItem::Events(evs);
                }
                PipeItem::TofParams { nt, dt, t0 } => {
                    self.nt = nt;
                    self.dt = dt;
                    self.t0 = t0;
                }
                PipeItem::State(ref module, ref state) => {
                    self.shm.set_state(*module, *state);
                }
            }
            self.output.send(item).expect("output sender closed");
        }
    }
}
