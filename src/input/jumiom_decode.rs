// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Decodes the Jumiom PSD's raw 32-bit-word list-mode stream into [`Event`]s.
//!
//! Bit-for-bit port of `Jumiom/LibHelper/jumiom_data_helper.c`'s
//! `jumpsd_fillhisto` for the `Tof1`, `Raw` and `Ramp` acquisition modes
//! (`Tof2` is not supported), with position mode 0 only (raw FPGA X/Y, no
//! ADC-ratio or distortion-correction math) and no per-pixel limit-table
//! filtering. Kept free of the `jumiom` feature gate so it can be unit
//! tested without `libjumpsd.so` installed.
#![cfg_attr(not(any(test, feature = "jumiom")), allow(dead_code))]

use crate::config::JumiomMode;
use crate::event::{Event, EventTime, EventType};

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
    use_gate: bool,
    /// Position within the current event; negative until the first framing
    /// word (top bit set) has been seen.
    index: i32,
    // Tof1 word0 fields
    xfpga: u8,
    yfpga: u8,
    gatebit: bool,
    bit31: bool,
    ignore_w2w3: u32,
    tof_counter: i64,
    adcval: [i32; 4],
    /// Scratch: word2 (Tof1) or the first word of a 2-word record (Raw/Ramp).
    word_a: u32,
}

impl JumiomDecoder {
    pub fn new(mode: JumiomMode, use_gate: bool) -> Self {
        Self {
            mode,
            use_gate,
            index: -1,
            xfpga: 0,
            yfpga: 0,
            gatebit: false,
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
    pub fn feed(&mut self, words: &[u32]) -> Vec<Event> {
        let mut events = Vec::new();
        for &word in words {
            match self.mode {
                JumiomMode::Tof1 => self.feed_tof1(word, &mut events),
                JumiomMode::Raw => self.feed_raw(word, &mut events),
                JumiomMode::Ramp => self.feed_ramp(word, &mut events),
            }
        }
        events
    }

    fn advance_index(&mut self, word: u32) {
        if word & 0x8000_0000 != 0 {
            self.index = 0;
        } else {
            self.index += 1;
        }
    }

    fn feed_tof1(&mut self, word: u32, events: &mut Vec<Event>) {
        self.advance_index(word);
        match self.index {
            i if i < 0 => {}
            0 => {
                self.xfpga = (word & 0xFF) as u8;
                self.yfpga = ((word >> 8) & 0xFF) as u8;
                self.gatebit = word & 0x4000_0000 != 0;
                self.bit31 = word & 0x2000_0000 != 0;
                self.ignore_w2w3 = word & 0x1800_0000;
            }
            1 => {
                let counter = if self.bit31 { word | 0x8000_0000 } else { word };
                self.tof_counter = counter as i64;
                let rel_time = EventTime::from_ticks(NS_PER_TICK, self.tof_counter);
                if self.ignore_w2w3 & 0x1000_0000 != 0 {
                    events.push(Event::new(EventType::Monitor).with_rel_time(rel_time));
                } else if self.ignore_w2w3 & 0x0800_0000 != 0 {
                    events.push(Event::new(EventType::Tzero).with_rel_time(rel_time));
                }
            }
            2 => {
                if self.ignore_w2w3 != 0 {
                    return;
                }
                self.word_a = word;
                self.adcval[0] = adc12(word, 0) >> 3;
                self.adcval[1] = adc12(word, 16) >> 3;
            }
            3 => {
                if self.ignore_w2w3 != 0 {
                    return;
                }
                let (word2, word3) = (self.word_a, word);
                self.adcval[2] = adc12(word3, 0) >> 3;
                self.adcval[3] = adc12(word3, 16) >> 3;
                if self.gatebit || (!self.use_gate) {
                    let sum: i32 = self.adcval.iter().copied().filter(|&v| v >= 0).sum();
                    let mut ev = Event::new(EventType::Neutron)
                        .with_ampl((sum >> 2) as u32)
                        .with_raw(word2, word3)
                        .with_rel_time(EventTime::from_ticks(NS_PER_TICK, self.tof_counter));
                    ev.histo.x = self.xfpga as u16;
                    ev.histo.y = self.yfpga as u16;
                    events.push(ev);
                }
            }
            _ => {}
        }
    }

    fn feed_raw(&mut self, word: u32, events: &mut Vec<Event>) {
        self.advance_index(word);
        match self.index {
            i if i < 0 => {}
            0 => self.word_a = word,
            1 => {
                let (word0, word1) = (self.word_a, word);
                let adc = [
                    (word0 & 0x0000_0FF0) >> 4,
                    (word0 & 0x0FF0_0000) >> 20,
                    (word1 & 0x0000_0FF0) >> 4,
                    (word1 & 0x0FF0_0000) >> 20,
                ];
                for (num, ampl) in adc.into_iter().enumerate() {
                    events.push(
                        Event::new(EventType::AuxSignal { num: num as u8 })
                            .with_ampl(ampl)
                            .with_raw(word0, word1),
                    );
                }
            }
            _ => {}
        }
    }

    fn feed_ramp(&mut self, word: u32, events: &mut Vec<Event>) {
        self.advance_index(word);
        match self.index {
            i if i < 0 => {}
            0 => self.word_a = word,
            1 => {
                let (word0, word1) = (self.word_a, word);
                let adc = [
                    (word0 & 0x7FFF_0000) >> 23,
                    (word0 & 0x0000_7FFF) >> 7,
                    (word1 & 0x7FFF_0000) >> 23,
                    (word1 & 0x0000_7FFF) >> 7,
                ];
                for (num, ampl) in adc.into_iter().enumerate() {
                    events.push(
                        Event::new(EventType::AuxSignal { num: num as u8 })
                            .with_ampl(ampl)
                            .with_raw(word0, word1),
                    );
                }
            }
            _ => {}
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
    fn test_tof1_gated_neutron_event() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let words = tof1_words(true, 5, 3, 1234, [100, 50, 25, 10]);
        let events = dec.feed(&words);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.evtype, EventType::Neutron);
        assert_eq!(ev.histo.x, 5);
        assert_eq!(ev.histo.y, 3);
        // (100>>3=12) + (50>>3=6) + (25>>3=3) + (10>>3=1) = 22, >>2 = 5
        assert_eq!(ev.ampl.0, 5);
        assert_eq!(ev.raw, (words[2], words[3]));
        assert_eq!(ev.rel_time, EventTime::from_ticks(NS_PER_TICK, 1234i64));
    }

    #[test]
    fn test_tof1_ungated_event_dropped_unless_ingore_gate() {
        let words = tof1_words(false, 1, 1, 10, [8, 8, 8, 8]);

        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, true);
        assert!(dec.feed(&words).is_empty());

        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let events = dec.feed(&words);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Neutron);
    }

    #[test]
    fn test_tof1_negative_adc_excluded_from_sum() {
        // adc[1] negative (e.g. junk/invalid reading): excluded from the sum,
        // not merely clamped to zero.
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let words = tof1_words(true, 0, 0, 0, [40, -1, 40, 40]);
        let events = dec.feed(&words);
        assert_eq!(events.len(), 1);
        // (40>>3=5)*3 = 15, >>2 = 3
        assert_eq!(events[0].ampl.0, 3);
    }

    #[test]
    fn test_tof1_event_split_across_feed_calls() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let words = tof1_words(true, 7, 9, 42, [16, 16, 16, 16]);

        assert!(dec.feed(&words[..2]).is_empty());
        assert!(dec.feed(&words[2..3]).is_empty());
        let events = dec.feed(&words[3..]);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].histo.x, 7);
        assert_eq!(events[0].histo.y, 9);
        assert_eq!(events[0].ampl.0, 2); // (16>>3=2)*4 >> 2 = 2
    }

    #[test]
    fn test_tof1_monitor_and_chopper_events() {
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let monitor_word0 = 0x8000_0000 | 0x1000_0000;
        let events = dec.feed(&[monitor_word0, 500]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].evtype, EventType::Monitor);
        assert_eq!(events[0].rel_time, EventTime::from_ticks(NS_PER_TICK, 500i64));

        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
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
        let mut dec = JumiomDecoder::new(JumiomMode::Tof1, false);
        let monitor_word0 = 0x8000_0000 | 0x1000_0000;
        let next = tof1_words(true, 2, 2, 1, [8, 8, 8, 8]);
        let mut stream = vec![monitor_word0, 1];
        stream.extend_from_slice(&next);
        let events = dec.feed(&stream);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].evtype, EventType::Monitor);
        assert_eq!(events[1].evtype, EventType::Neutron);
        assert_eq!(events[1].histo.x, 2);
    }

    #[test]
    fn test_raw_mode_four_channels() {
        let mut dec = JumiomDecoder::new(JumiomMode::Raw, false);
        let word0 = 0x8000_0000 | (0xCDu32 << 20) | (0xABu32 << 4);
        let word1 = (0x12u32 << 20) | (0x34u32 << 4);
        let events = dec.feed(&[word0, word1]);
        assert_eq!(events.len(), 4);
        let expect = [0xAB, 0xCD, 0x34, 0x12];
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(ev.evtype, EventType::AuxSignal { num: i as u8 });
            assert_eq!(ev.ampl.0, expect[i]);
            assert_eq!(ev.raw, (word0, word1));
        }
    }

    #[test]
    fn test_ramp_mode_four_channels() {
        let mut dec = JumiomDecoder::new(JumiomMode::Ramp, false);
        let word0 = 0x8000_0000 | (0x1234u32 << 16) | 0x5678u32;
        let word1 = (0x2222u32 << 16) | 0x3333u32;
        let events = dec.feed(&[word0, word1]);
        assert_eq!(events.len(), 4);
        // pre-computed: 0x1234>>7=36, 0x5678>>7=172, 0x2222>>7=68, 0x3333>>7=102
        let expect = [36, 172, 68, 102];
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(ev.evtype, EventType::AuxSignal { num: i as u8 });
            assert_eq!(ev.ampl.0, expect[i]);
        }
    }
}
