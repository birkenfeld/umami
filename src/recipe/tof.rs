// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use crate::config::RecipeConfig;
use crate::event::{Event, EventData, EventFlags, EventTime};
use crate::recipe::Recipe;

pub struct TofStd {
    last_t0: EventTime,
}

// TODO: remember multiple t0s for multiple inputs, and try to find the closest one

impl Recipe for TofStd {
    type Config = ();

    fn from_config(_: Self::Config, _: &BTreeMap<String, RecipeConfig>) -> Self {
        TofStd { last_t0: EventTime::zero() }
    }

    fn process(&mut self, mut events: Vec<Event>) -> Vec<Event> {
        for event in &mut events {
            if let EventData::Tzero = event.data {
                self.last_t0 = event.time;
            } else {
                if !event.flags.contains(EventFlags::HasRelTime) {
                    event.rel_time = event.time - self.last_t0;
                    event.flags.set(EventFlags::HasRelTime);
                }
            }
        }
        events
    }
}
