// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use crate::channel::{Sender, Receiver};
use crate::event::Event;

pub struct Sorter {
    pub rcv1: Receiver<Vec<Event>>,
    pub rcv2: Receiver<Vec<Event>>,
    pub send: Sender<Vec<Event>>,
}

impl Sorter {
    pub fn run(rcv1: Receiver<Vec<Event>>, rcv2: Receiver<Vec<Event>>, send: Sender<Vec<Event>>) {
        std::thread::spawn(move || {
            let mut sorter = Sorter { rcv1, rcv2, send };
            sorter.main();
        });
    }

    fn main(&mut self) {
        let mut buffer1: Vec<Event> = Vec::with_capacity(1024);
        let mut buffer2: Vec<Event> = Vec::with_capacity(1024);
        // let mut ts = crate::event::EventTime::zero();

        loop {
            if buffer1.is_empty() {
                match self.rcv1.recv() {
                    Ok(evs) => buffer1 = evs,
                    Err(_) => {
                        self.send.send(buffer2).unwrap();
                        while let Ok(evs) = self.rcv2.recv() {
                            self.send.send(evs).unwrap();
                        }
                        return;
                    }
                }
            }
            if buffer2.is_empty() {
                match self.rcv2.recv() {
                    Ok(evs) => buffer2 = evs,
                    Err(_) => if buffer2.is_empty() {
                        self.send.send(buffer1).unwrap();
                        while let Ok(evs) = self.rcv1.recv() {
                            self.send.send(evs).unwrap();
                        }
                        return;
                    }
                }
            }
            if buffer1.is_empty() || buffer2.is_empty() {
                continue;
            }
            let last1 = buffer1.last().unwrap().time;
            let last2 = buffer2.last().unwrap().time;
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
            self.send.send(batch).unwrap();
        }
    }
}
