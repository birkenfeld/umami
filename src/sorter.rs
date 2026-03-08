// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use crate::channel::{Sender, Receiver};
use crate::error::UResult;
use crate::event::{Event, EventData};

pub struct Sorter {
    pub rcv1: Receiver<Vec<Event>>,
    pub rcv2: Receiver<Vec<Event>>,
    pub send: Sender<Vec<Event>>,
    buffer1: Vec<Event>,
    buffer2: Vec<Event>,
}

fn is_end(evs: &[Event]) -> bool {
    evs.last().map_or(false, |ev| matches!(ev.data, EventData::EndOfRun))
}

impl Sorter {
    pub fn new(rcv1: Receiver<Vec<Event>>, rcv2: Receiver<Vec<Event>>, send: Sender<Vec<Event>>) -> Self {
        Sorter { rcv1, rcv2, send, buffer1: Vec::new(), buffer2: Vec::new() }
    }

    pub fn start(self) -> UResult<()> {
        std::thread::Builder::new()
            .name("Sorter".into())
            .spawn(move || self.main())
            .context("Spawning sorter thread")?;
        Ok(())
    }

    fn main(mut self) {
        // let mut ts = crate::event::EventTime::zero();

        'main: loop {
            if self.buffer1.is_empty() {
                match self.rcv1.recv() {
                    Ok(evs) => self.buffer1 = evs,
                    Err(_) => return,
                }
                if is_end(&self.buffer1) {
                    while let Ok(evs) = self.rcv2.recv() {
                        if is_end(&evs) {
                            // start anew
                            self.buffer1.clear();
                            self.buffer2.clear();
                            self.send.send(evs).unwrap();
                            continue 'main;
                        }
                        self.send.send(evs).unwrap();
                    }
                }
            }
            if self.buffer2.is_empty() {
                // TODO: duplicate
                match self.rcv2.recv() {
                    Ok(evs) => self.buffer2 = evs,
                    Err(_) => return,
                }
                if is_end(&self.buffer2) {
                    while let Ok(evs) = self.rcv1.recv() {
                        if is_end(&evs) {
                            // start anew
                            self.buffer1.clear();
                            self.buffer2.clear();
                            self.send.send(evs).unwrap();
                            continue 'main;
                        }
                        self.send.send(evs).unwrap();
                    }
                }
            }
            if self.buffer1.is_empty() || self.buffer2.is_empty() {
                continue;
            }
            let last1 = self.buffer1.last().unwrap().time;
            let last2 = self.buffer2.last().unwrap().time;
            // println!("{:?} bufferlen {} {}, lasttime {} {} {}", std::thread::current().id(),
            //          self.buffer1.len(), self.buffer2.len(), last1, last2, last1 - last2);
            let mut batch = if last1 < last2 {
                if let Some(stop_index) = self.buffer2.iter().rposition(|ev| ev.time < last1) {
                    // println!("{:?} Taking {} of {} from buffer2", std::thread::current().id(),
                    //      stop_index+1, self.buffer2.len());
                    self.buffer1.extend(self.buffer2.drain(0..=stop_index));
                }
                std::mem::replace(&mut self.buffer1, Vec::with_capacity(1024))
            } else {
                if let Some(stop_index) = self.buffer1.iter().rposition(|ev| ev.time < last2) {
                    // println!("{:?} Taking {} of {} from buffer1", std::thread::current().id(),
                    //      stop_index+1, self.buffer1.len());
                    self.buffer2.extend(self.buffer1.drain(0..=stop_index));
                }
                std::mem::replace(&mut self.buffer2, Vec::with_capacity(1024))
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
            self.send.send(batch).unwrap();
        }
    }
}
