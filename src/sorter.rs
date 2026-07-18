// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use crate::ltrace;
use crate::channel::{Sender, Receiver};
use crate::error::UResult;
use crate::event::Event;
use crate::pipeline::PipeItem;

pub struct Sorter {
    pub recv1: Receiver<PipeItem>,
    pub recv2: Receiver<PipeItem>,
    pub send: Sender<PipeItem>,
}

impl Sorter {
    pub fn new(recv1: Receiver<PipeItem>, recv2: Receiver<PipeItem>, send: Sender<PipeItem>) -> Self {
        Sorter { recv1, recv2, send }
    }

    pub fn start(self) -> UResult<()> {
        std::thread::Builder::new()
            .name("Sorter".into())
            .spawn(move || self.main())
            .context("Spawning sorter thread")?;
        Ok(())
    }

    fn refill_buffer(
        &self,
        fill_recv: &Receiver<PipeItem>,
        other_recv: &Receiver<PipeItem>,
        other_buf: &mut Vec<Event>,
    ) -> Option<Vec<Event>> {
        match fill_recv.recv() {
            Ok(PipeItem::EndOfRun) => {
                self.send.send(PipeItem::Events(std::mem::take(other_buf))).ok()?;
                ltrace!("Sorter received end from one channel");
                while let Ok(item_b) = other_recv.recv() {
                    match item_b {
                        PipeItem::EndOfRun => {
                            ltrace!("Sorter received end from other channel");
                            // send on one of the end events
                            self.send.send(PipeItem::EndOfRun).ok()?;
                            return Some(vec![]);
                        }
                        _ => self.send.send(item_b).ok()?,
                    }
                }
                None  // input channel closed
            }
            Ok(PipeItem::Events(evs_a)) => Some(evs_a),
            Ok(item) => {
                self.send.send(item).ok()?;
                self.refill_buffer(fill_recv, other_recv, other_buf)  // try again
            }
            Err(_) => None,
        }
    }

    fn main(self) {
        let mut buffer1 = vec![];
        let mut buffer2 = vec![];

        loop {
            if buffer1.is_empty() {
                match self.refill_buffer(&self.recv1, &self.recv2, &mut buffer2) {
                    Some(evs) => buffer1 = evs,
                    None => return,
                }
            }
            if buffer2.is_empty() {
                match self.refill_buffer(&self.recv2, &self.recv1, &mut buffer1) {
                    Some(evs) => buffer2 = evs,
                    None => return,
                }
            }
            if buffer1.is_empty() || buffer2.is_empty() {
                continue;
            }
            let last1 = buffer1.last().expect("not empty").time;
            let last2 = buffer2.last().expect("not empty").time;
            let mut batch = if last1 < last2 {
                if let Some(stop_index) = buffer2.iter().rposition(|ev| ev.time < last1) {
                    buffer1.extend(buffer2.drain(0..=stop_index));
                }
                std::mem::replace(&mut buffer1, Vec::with_capacity(1024))
            } else {
                if let Some(stop_index) = buffer1.iter().rposition(|ev| ev.time < last2) {
                    buffer2.extend(buffer1.drain(0..=stop_index));
                }
                std::mem::replace(&mut buffer2, Vec::with_capacity(1024))
            };
            batch.sort();
            if self.send.send(PipeItem::Events(batch)).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;
    use crate::channel;

    fn make_sorter() -> (channel::Sender<PipeItem>, channel::Sender<PipeItem>,
                         channel::Receiver<PipeItem>) {
        let (s1, r1) = channel::unbounded();
        let (s2, r2) = channel::unbounded();
        let (out_s, out_r) = channel::unbounded();
        let sorter = Sorter::new(r1, r2, out_s);
        std::thread::spawn(move || sorter.main());
        (s1, s2, out_r)
    }

    fn recv_all(recv: &channel::Receiver<PipeItem>) -> Vec<Event> {
        let mut all = vec![];
        while let Ok(item) = recv.try_recv() {
            if let PipeItem::Events(evs) = item {
                all.extend(evs);
            }
        }
        all
    }

    #[test]
    fn test_sorter_merge_sorted_streams() {
        let (s1, s2, out) = make_sorter();
        s1.send(PipeItem::Events(vec![
            test_utils::neutron(100, 0),
            test_utils::neutron(300, 0),
            test_utils::neutron(500, 0),
        ])).unwrap();
        s2.send(PipeItem::Events(vec![
            test_utils::neutron(200, 0),
            test_utils::neutron(400, 0),
        ])).unwrap();
        // signal end on both
        s1.send(PipeItem::EndOfRun).unwrap();
        s2.send(PipeItem::EndOfRun).unwrap();

        // give sorter time to process
        std::thread::sleep(std::time::Duration::from_millis(50));
        let events = recv_all(&out);
        let times: Vec<i64> = events.iter().map(|e: &Event| e.time.0).collect();
        assert_eq!(times, vec![100, 200, 300, 400, 500]);
    }

    #[test]
    fn test_sorter_passthrough_non_events() {
        let (s1, s2, out) = make_sorter();
        s1.send(PipeItem::StartOfRun("run1".into())).unwrap();
        s2.send(PipeItem::Events(vec![test_utils::neutron(100, 0)])).unwrap();
        s1.send(PipeItem::Events(vec![test_utils::neutron(200, 0)])).unwrap();
        s1.send(PipeItem::EndOfRun).unwrap();
        s2.send(PipeItem::EndOfRun).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut items = vec![];
        while let Ok(item) = out.try_recv() {
            items.push(item);
        }
        // StartOfRun should pass through
        assert!(items.iter().any(|i| matches!(i, PipeItem::StartOfRun(_))));
        // events should be merged and sorted
        let mut events = vec![];
        for item in &items {
            if let PipeItem::Events(evs) = item {
                events.extend(evs);
            }
        }
        let times: Vec<i64> = events.iter().map(|e: &Event| e.time.0).collect();
        assert_eq!(times, vec![100, 200]);
    }

    #[test]
    fn test_sorter_end_of_run_forwards() {
        let (s1, s2, out) = make_sorter();
        s1.send(PipeItem::EndOfRun).unwrap();
        s2.send(PipeItem::EndOfRun).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        let items: Vec<_> = out.try_iter().collect();
        assert!(items.iter().any(|i| matches!(i, PipeItem::EndOfRun)));
    }
}
