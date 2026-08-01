// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Decodes the Jumiom PSD's raw 32-bit-word list-mode stream into [`Event`]s.
//!
//! Bit-for-bit port of `Jumiom/LibHelper/jumiom_data_helper.c`'s
//! `jumpsd_fillhisto` for the `Tof1`, `Raw` and `Ramp` acquisition modes
//! (`Tof2` is not supported). No position calculation happens here: a
//! Tof1 Neutron event's raw FPGA X/Y is encoded into `Event.channel` (X in
//! the low byte, Y in the next byte), decoded into `histo.x`/`histo.y` by
//! the `jumiom` recipe (`src/recipe/jumiom.rs`) instead.
//!
//! Gate filtering isn't done here either: Tof1 has no separate gate-signal
//! word, just a bit riding along on every Neutron event, so a `Gate` event
//! is synthesized on each transition and every Neutron event is emitted
//! unconditionally -- `histo_std`/`histo_tof`'s own `use_gate` handles the
//! filtering, same as for every other detector.

use crate::event::{Event, EventTime, EventType};
use super::JumiomMode;

/// Each TOF1 tick is one microsecond (confirmed hardware constant).
const NS_PER_TICK: i64 = 1_000;

/// Sign-extends the 12-bit field at `shift` in `word` to a full `i32`.
fn adc12(word: u32, shift: u32) -> i32 {
    (((word >> shift) & 0xFFF) as i32) << 20 >> 20
}

/// Stateful decoder for one Jumiom acquisition stream. Word framing (and, for
/// `Tof1`, in-progress event fields) persists across [`feed`](Self::feed)
/// calls, since a single event's words can be split across separate DMA
/// chunks.
pub struct JumiomDecoder {
    mode: JumiomMode,
    /// Position within the current event, resynced to 0 whenever a word
    /// with the top bit set (a new frame) is seen.
    index: i32,
    // Tof1 word0 fields
    xfpga: u8,
    yfpga: u8,
    gatebit: bool,
    /// Last gate state a `Gate` event was emitted for, so one is only
    /// synthesized on an actual transition (Tof1 has no separate gate-signal
    /// word on the wire -- the bit rides along on every Neutron event).
    last_gatebit: bool,
    bit31: bool,
    ignore_w2w3: u32,
    tof_counter: i64,
    adcval: [i32; 4],
    /// Scratch: word2 (Tof1) or the first word of a 2-word record (Raw/Ramp).
    word_a: u32,
}

impl JumiomDecoder {
    pub fn new(mode: JumiomMode) -> Self {
        Self {
            mode,
            index: -1,
            xfpga: 0,
            yfpga: 0,
            gatebit: false,
            last_gatebit: false,
            bit31: false,
            ignore_w2w3: 0,
            tof_counter: 0,
            adcval: [0; 4],
            word_a: 0,
        }
    }

    /// Decodes a chunk of native-endian 32-bit words, returning any events
    /// completed by this chunk (an event started in a previous chunk may
    /// complete here, and one started here may only complete in the next).
    /// Each word yields at most one event.
    pub fn feed(&mut self, words: &[u32]) -> Vec<Event> {
        words.iter().filter_map(|&word| match self.mode {
            JumiomMode::Tof1 => self.feed_tof1(word),
            JumiomMode::Raw => self.feed_raw(word),
            JumiomMode::Ramp => self.feed_ramp(word),
        }).collect()
    }

    fn advance_index(&mut self, word: u32) {
        if word & 0x8000_0000 != 0 {
            self.index = 0;
        } else {
            self.index += 1;
        }
    }

    fn feed_tof1(&mut self, word: u32) -> Option<Event> {
        self.advance_index(word);
        match self.index {
            0 => {
                self.xfpga = (word & 0xFF) as u8;
                self.yfpga = ((word >> 8) & 0xFF) as u8;
                self.gatebit = word & 0x4000_0000 != 0;
                self.bit31 = word & 0x2000_0000 != 0;
                self.ignore_w2w3 = word & 0x1800_0000;
                if self.gatebit != self.last_gatebit {
                    self.last_gatebit = self.gatebit;
                    Some(Event::new(EventType::Gate { up: self.gatebit }))
                } else {
                    None
                }
            }
            1 => {
                let counter = if self.bit31 { word | 0x8000_0000 } else { word };
                self.tof_counter = counter as i64;
                let rel_time = EventTime::from_ticks(NS_PER_TICK, self.tof_counter);
                if self.ignore_w2w3 & 0x1000_0000 != 0 {
                    Some(Event::new(EventType::Monitor).with_rel_time(rel_time))
                } else if self.ignore_w2w3 & 0x0800_0000 != 0 {
                    Some(Event::new(EventType::Tzero).with_rel_time(rel_time))
                } else {
                    None
                }
            }
            2 => {
                if self.ignore_w2w3 == 0 {
                    self.word_a = word;
                    self.adcval[0] = adc12(word, 0) >> 3;
                    self.adcval[1] = adc12(word, 16) >> 3;
                }
                None
            }
            3 => {
                if self.ignore_w2w3 != 0 {
                    return None;
                }
                let (word2, word3) = (self.word_a, word);
                self.adcval[2] = adc12(word3, 0) >> 3;
                self.adcval[3] = adc12(word3, 16) >> 3;
                let sum: i32 = self.adcval.iter().copied().filter(|&v| v >= 0).sum();
                let channel = (self.yfpga as u32) << 8 | self.xfpga as u32;
                Some(Event::new(EventType::Neutron)
                    .with_channel(channel)
                    .with_ampl((sum >> 2) as u32)
                    .with_raw(word2, word3)
                    .with_rel_time(EventTime::from_ticks(NS_PER_TICK, self.tof_counter)))
            }
            _ => None,
        }
    }

    fn feed_raw(&mut self, word: u32) -> Option<Event> {
        self.advance_index(word);
        match self.index {
            0 => { self.word_a = word; None }
            1 => {
                let (word0, word1) = (self.word_a, word);
                let sum = ((word0 & 0x0000_0FF0) >> 4)
                    + ((word0 & 0x0FF0_0000) >> 20)
                    + ((word1 & 0x0000_0FF0) >> 4)
                    + ((word1 & 0x0FF0_0000) >> 20);
                Some(Event::new(EventType::AuxSignal { num: 0 })
                    .with_ampl(sum >> 2)
                    .with_raw(word0, word1))
            }
            _ => None,
        }
    }

    fn feed_ramp(&mut self, word: u32) -> Option<Event> {
        self.advance_index(word);
        match self.index {
            0 => { self.word_a = word; None }
            1 => {
                let (word0, word1) = (self.word_a, word);
                let sum = ((word0 & 0x7FFF_0000) >> 23)
                    + ((word0 & 0x0000_7FFF) >> 7)
                    + ((word1 & 0x7FFF_0000) >> 23)
                    + ((word1 & 0x0000_7FFF) >> 7);
                Some(Event::new(EventType::AuxSignal { num: 1 })
                    .with_ampl(sum >> 2)
                    .with_raw(word0, word1))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tof1_words(gatebit: bool, x: u8, y: u8, counter: u32, adc: [i32; 4]) -> [u32; 4] {
        let word0 = 0x8000_0000
            | if gatebit { 0x4000_0000 } else { 0 }
            | ((y as u32) << 8)
            | x as u32;
        let word1 = counter;
        let enc = |v: i32| -> u32 { (v as u32) & 0xFFF };
        let word2 = (enc(adc[1]) << 16) | enc(adc[0]);
        let word3 = (enc(adc[3]) << 16) | enc(adc[2]);
        [word0, word1, word2, word3]
    }

    #[test]
    fn test_tof1_neutron_event_and_gate_transition() {
        // gatebit=true from a fresh decoder (last_gatebit starts false) is a
        // transition, so a Gate event precedes the Neutron.
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let words = tof1_words(true, 5, 3, 1234, [100, 50, 25, 10]);
        let events = dec.feed(&words);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].evtype, EventType::Gate { up: true });
        let ev = &events[1];
        assert_eq!(ev.evtype, EventType::Neutron);
        assert_eq!(ev.channel.0 & 0xFF, 5); // x
        assert_eq!((ev.channel.0 >> 8) & 0xFF, 3); // y
        // (100>>3=12) + (50>>3=6) + (25>>3=3) + (10>>3=1) = 22, >>2 = 5
        assert_eq!(ev.ampl.0, 5);
        assert_eq!(ev.raw, (words[2], words[3]));
        assert_eq!(ev.rel_time, EventTime::from_ticks(NS_PER_TICK, 1234i64));
    }

    #[test]
    fn test_tof1_neutron_always_emitted_regardless_of_gate() {
        // gatebit=false matches the initial last_gatebit=false: no Gate
        // event, but the Neutron event is still emitted (no filtering here
        // any more -- histo_std/histo_tof's use_gate does that).
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let words = tof1_words(false, 1, 1, 10, [8, 8, 8, 8]);
        let events = dec.feed(&words);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Neutron);
    }

    #[test]
    fn test_tof1_gate_event_only_on_transition() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let gated = tof1_words(true, 1, 1, 10, [8, 8, 8, 8]);

        let events = dec.feed(&gated);
        assert_eq!(events.len(), 2); // Gate{true} + Neutron
        assert_eq!(events[0].evtype, EventType::Gate { up: true });

        // same gate state again: no repeated Gate event, just the Neutron
        let events = dec.feed(&gated);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Neutron);
    }

    #[test]
    fn test_tof1_negative_adc_excluded_from_sum() {
        // adc[1] negative (e.g. junk/invalid reading): excluded from the sum,
        // not merely clamped to zero.
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let words = tof1_words(true, 0, 0, 0, [40, -1, 40, 40]);
        let events = dec.feed(&words);
        let neutron = events.iter().find(|e| e.evtype == EventType::Neutron).unwrap();
        // (40>>3=5)*3 = 15, >>2 = 3
        assert_eq!(neutron.ampl.0, 3);
    }

    #[test]
    fn test_tof1_event_split_across_feed_calls() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let words = tof1_words(true, 7, 9, 42, [16, 16, 16, 16]);

        // word0 alone already yields the gate transition
        let first = dec.feed(&words[..2]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].evtype, EventType::Gate { up: true });

        assert!(dec.feed(&words[2..3]).is_empty());
        let events = dec.feed(&words[3..]);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Neutron);
        assert_eq!(events[0].channel.0 & 0xFF, 7); // x
        assert_eq!((events[0].channel.0 >> 8) & 0xFF, 9); // y
        assert_eq!(events[0].ampl.0, 2); // (16>>3=2)*4 >> 2 = 2
    }

    #[test]
    fn test_tof1_monitor_and_chopper_events() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let monitor_word0 = 0x8000_0000 | 0x1000_0000;
        let events = dec.feed(&[monitor_word0, 500]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Monitor);
        assert_eq!(events[0].rel_time, EventTime::from_ticks(NS_PER_TICK, 500i64));

        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let chopper_word0 = 0x8000_0000 | 0x0800_0000;
        let events = dec.feed(&[chopper_word0, 777]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Tzero);
        assert_eq!(events[0].rel_time, EventTime::from_ticks(NS_PER_TICK, 777i64));
    }

    #[test]
    fn test_tof1_monitor_event_immediately_followed_by_next_frame() {
        // Monitor/Chopper events are only 2 words on the wire; the very next
        // word is already the next event's word0, and framing must resync
        // correctly without needing to see word2/word3 first.
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1);
        let monitor_word0 = 0x8000_0000 | 0x1000_0000;
        let next = tof1_words(true, 2, 2, 1, [8, 8, 8, 8]);
        let mut stream = vec![monitor_word0, 1];
        stream.extend_from_slice(&next);
        let events = dec.feed(&stream);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].evtype, EventType::Monitor);
        assert_eq!(events[1].evtype, EventType::Gate { up: true });
        assert_eq!(events[2].evtype, EventType::Neutron);
        assert_eq!(events[2].channel.0 & 0xFF, 2); // x
    }

    #[test]
    fn test_raw_mode_single_averaged_event() {
        let mut dec = JumiomDecoder::new(JumiomMode::Raw);
        let word0 = 0x8000_0000 | (0xCDu32 << 20) | (0xABu32 << 4);
        let word1 = (0x12u32 << 20) | (0x34u32 << 4);
        let events = dec.feed(&[word0, word1]);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.evtype, EventType::AuxSignal { num: 0 });
        // (0xAB=171 + 0xCD=205 + 0x34=52 + 0x12=18) = 446, >>2 = 111
        assert_eq!(ev.ampl.0, 111);
        assert_eq!(ev.raw, (word0, word1));
    }

    #[test]
    fn test_ramp_mode_single_averaged_event() {
        let mut dec = JumiomDecoder::new(JumiomMode::Ramp);
        let word0 = 0x8000_0000 | (0x1234u32 << 16) | 0x5678u32;
        let word1 = (0x2222u32 << 16) | 0x3333u32;
        let events = dec.feed(&[word0, word1]);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.evtype, EventType::AuxSignal { num: 1 });
        // per-channel values (see prior per-channel test derivation): 36, 172, 68, 102
        // sum = 378, >>2 = 94
        assert_eq!(ev.ampl.0, 94);
        assert_eq!(ev.raw, (word0, word1));
    }
}
