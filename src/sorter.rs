// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use crate::ltrace;
use crate::channel::{Sender, Receiver};
use crate::error::UResult;
use crate::event::Event;
use crate::pipeline::PipeItem;

pub struct Sorter {
    pub rcv1: Receiver<PipeItem>,
    pub rcv2: Receiver<PipeItem>,
    pub send: Sender<PipeItem>,
}

impl Sorter {
    pub fn new(rcv1: Receiver<PipeItem>, rcv2: Receiver<PipeItem>, send: Sender<PipeItem>) -> Self {
        Sorter { rcv1, rcv2, send }
    }

    pub fn start(self) -> UResult<()> {
        std::thread::Builder::new()
            .name("Sorter".into())
            .spawn(move || self.main())
            .context("Spawning sorter thread")?;
        Ok(())
    }

    fn refill_buffer(rcv_a: &Receiver<PipeItem>,
                     rcv_b: &Receiver<PipeItem>,
                     snd: &Sender<PipeItem>,
                     buf_b: &mut Vec<Event>) -> Option<Vec<Event>> {
        match rcv_a.recv() {
            Ok(PipeItem::EndOfRun) => {
                snd.send(PipeItem::Events(std::mem::take(buf_b))).unwrap();
                ltrace!("Sorter received end from one channel");
                while let Ok(item_b) = rcv_b.recv() {
                    match item_b {
                        PipeItem::EndOfRun => {
                            ltrace!("Sorter received end from other channel");
                            // send on one of the end events
                            snd.send(PipeItem::EndOfRun).unwrap();
                            return Some(vec![]);
                        }
                        _ => snd.send(item_b).unwrap(),
                    }
                }
                None  // input channel closed
            }
            Ok(PipeItem::Events(evs_a)) => Some(evs_a),
            Ok(item) => {
                snd.send(item).unwrap();
                Self::refill_buffer(rcv_a, rcv_b, snd, buf_b)  // try again
            }
            Err(_) => None,
        }
    }

    fn main(self) {
        let mut buffer1 = vec![];
        let mut buffer2 = vec![];
        // let mut ts = crate::event::EventTime::zero();

        loop {
            if buffer1.is_empty() {
                match Self::refill_buffer(&self.rcv1, &self.rcv2, &self.send, &mut buffer2) {
                    Some(evs) => buffer1 = evs,
                    None => return,
                }
            }
            if buffer2.is_empty() {
                match Self::refill_buffer(&self.rcv2, &self.rcv1, &self.send, &mut buffer1) {
                    Some(evs) => buffer2 = evs,
                    None => return,
                }
            }
            if buffer1.is_empty() || buffer2.is_empty() {
                continue;
            }
            let last1 = buffer1.last().expect("not empty").time;
            let last2 = buffer2.last().expect("not empty").time;
            // println!("{:?} bufferlen {} {}, lasttime {} {} {}", std::thread::current().id(),
            //          buffer1.len(), buffer2.len(), last1, last2, last1 - last2);
            let mut batch = if last1 < last2 {
                if let Some(stop_index) = buffer2.iter().rposition(|ev| ev.time < last1) {
                    // println!("{:?} Taking {} of {} from buffer2", std::thread::current().id(),
                    //      stop_index+1, buffer2.len());
                    buffer1.extend(buffer2.drain(0..=stop_index));
                }
                std::mem::replace(&mut buffer1, Vec::with_capacity(1024))
            } else {
                if let Some(stop_index) = buffer1.iter().rposition(|ev| ev.time < last2) {
                    // println!("{:?} Taking {} of {} from buffer1", std::thread::current().id(),
                    //      stop_index+1, buffer1.len());
                    buffer2.extend(buffer1.drain(0..=stop_index));
                }
                std::mem::replace(&mut buffer2, Vec::with_capacity(1024))
            };
            batch.sort();
            // println!("{:?} out bufferlen {}, lasttime {}", std::thread::current().id(),
            //          batch.len(), batch.last().unwrap().time);
            // if batch[0].time < ts {
            //     println!("{:?} Received out-of-order batch with time {} (current ts {})",
            //              std::thread::current().id(),
            //              batch[0].time, ts);
            // }
            // ts = batch.last().unwrap().time;
            self.send.send(PipeItem::Events(batch)).unwrap();
        }
    }
}
