// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Jumiom PSD input, driven through `libjumpsd.so`.  We reuse its
//! `wrapped_jumpsd_dma()` acquisition loop as-is and provide the two callbacks
//! it expects (`jumpsd_fillhisto`, `jumpsd_setup_callback`), decoding into
//! [`Event`]s via [`decode::JumiomDecoder`].
//!
//! `wrapped_jumpsd_dma`'s callbacks carry no device-id/context parameter, so
//! only one Jumiom input can be active per umami process; this is enforced
//! via the `SHARED` static (see [`JumiomInput::start`]).

mod decode;

use std::ffi::{c_char, c_int};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use crate::channel::{Receiver, RecvTimeoutError};
use crate::command::{Command, CommandReply, ModuleId};
use crate::error::{UError, UResult};
use crate::event::Event;
use crate::lprintln;
use crate::input::{DumpHandler, Input, InputCommon};
use crate::params::HasParams;

/// Acquisition mode for the Jumiom PSD (selects both the hardware mode set
/// up via `jumpsd_set_*_mode` and the raw word-stream decoding). `Tof2` is
/// intentionally not supported (see `decode.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JumiomMode {
    Tof1,
    Raw,
    Ramp,
}

#[derive(Debug, Deserialize)]
pub struct JumiomConfig {
    /// Device number, i.e. `/dev/jumpsd_d<device>`.
    pub device: i32,
    pub mode: JumiomMode,
    /// Hardware calibration to push at acquisition start, matching what
    /// `jumiom_dma_wrapper`'s startup sequence used to write from
    /// `globalData.gp` when `loadCard` was set. If unset, umami leaves the
    /// hardware's current settings untouched (like `loadCard = 0`).
    #[serde(default)]
    pub calibration: Option<JumiomCalibration>,
}

/// Hardware calibration values for the Jumiom PSD, pushed via the
/// `jumpsd_write_*` API in `jumpsd_setup_callback`. Grouped to match how
/// they're set together in the field's `entangle` config
/// (`jumiom_det.ImageChannel`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct JumiomCalibration {
    /// Upper/lower/gate ADC thresholds (`jumpsd_write_threshold` levels 0..2).
    pub thresholds: [i32; 3],
    /// Gain potentiometer setting per ADC channel (`jumpsd_write_poti`).
    pub poti: [i32; 4],
    /// DAC offset per ADC channel, single-ended (`jumpsd_write_dac`).
    pub dac1: [i32; 4],
    /// DAC offset per ADC channel, differential (`jumpsd_write_dac2`).
    pub dac2: [i32; 4],
    /// Pileup rejection count.
    pub pileup: i32,
    /// Monitor timer reset delay [us] (`jumpsd_write_monitor_delay`).
    /// Monitor recording is always enabled, regardless of this block.
    #[serde(default)]
    pub monitor_delay: i32,
    /// Chopper timer reset delay [us] (`jumpsd_write_chopper_delay`).
    /// Chopper recording is always enabled, regardless of this block.
    #[serde(default)]
    pub chopper_delay: i32,
}

unsafe extern "C" {
    fn wrapped_jumpsd_dma(dev: c_int) -> c_int;
    fn write_status(fd: c_int, data: c_int) -> c_int;
    fn jumpsd_set_tof1_mode(fd: c_int, adcs: c_int) -> c_int;
    fn jumpsd_set_raw_mode(fd: c_int, adcs: c_int) -> c_int;
    fn jumpsd_set_ramp_mode(fd: c_int) -> c_int;
    fn jumpsd_write_threshold(fd: c_int, level: c_int, data: c_int) -> c_int;
    fn jumpsd_write_poti(fd: c_int, channel: c_int, data: c_int) -> c_int;
    fn jumpsd_write_dac(fd: c_int, level: c_int, data: c_int) -> c_int;
    fn jumpsd_write_dac2(fd: c_int, level: c_int, data: c_int) -> c_int;
    fn jumpsd_write_pileup(fd: c_int, data: c_int) -> c_int;
    fn jumpsd_set_monitor(fd: c_int, set: c_int) -> c_int;
    fn jumpsd_set_chopper(fd: c_int, set: c_int) -> c_int;
    fn jumpsd_write_monitor_delay(fd: c_int, data: c_int) -> c_int;
    fn jumpsd_write_chopper_delay(fd: c_int, data: c_int) -> c_int;
}

/// DMA break/stop bit, see `DriverJumiom/inc/jumpsd_var.h`.
const DMA_BREAK: c_int = 0x04;

struct Shared {
    mode: JumiomMode,
    calibration: Option<JumiomCalibration>,
    decoder: decode::JumiomDecoder,
    dump: DumpHandler,
    sender: crate::channel::Sender<Vec<Event>>,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
/// Device fd stashed by `jumpsd_setup_callback`, used by `stop()` to signal
/// the DMA loop to break out (the `dma_status` bits it checks live in the
/// driver's per-device state, so any fd for that minor works).
static FD: AtomicI32 = AtomicI32::new(-1);

#[unsafe(no_mangle)]
extern "C" fn jumpsd_setup_callback(fd: c_int) {
    FD.store(fd, Ordering::SeqCst);
    let Some((mode, calibration)) = SHARED.lock().unwrap()
        .as_ref().map(|s| (s.mode, s.calibration)) else { return };
    unsafe {
        match mode {
            JumiomMode::Tof1 => { jumpsd_set_tof1_mode(fd, 0xF); }
            JumiomMode::Raw => { jumpsd_set_raw_mode(fd, 0xF); }
            JumiomMode::Ramp => { jumpsd_set_ramp_mode(fd); }
        }
        if let Some(cal) = calibration {
            for (level, &data) in cal.thresholds.iter().enumerate() {
                jumpsd_write_threshold(fd, level as c_int, data);
            }
            for (channel, &data) in cal.poti.iter().enumerate() {
                jumpsd_write_poti(fd, channel as c_int, data);
            }
            for (level, &data) in cal.dac1.iter().enumerate() {
                jumpsd_write_dac(fd, level as c_int, data);
            }
            for (level, &data) in cal.dac2.iter().enumerate() {
                jumpsd_write_dac2(fd, level as c_int, data);
            }
            jumpsd_write_pileup(fd, cal.pileup);
        }
        let (monitor_delay, chopper_delay) = calibration
            .map(|cal| (cal.monitor_delay, cal.chopper_delay))
            .unwrap_or((0, 0));
        jumpsd_set_monitor(fd, 1);
        jumpsd_write_monitor_delay(fd, monitor_delay);
        jumpsd_set_chopper(fd, 1);
        jumpsd_write_chopper_delay(fd, chopper_delay);
    }
}

#[unsafe(no_mangle)]
extern "C" fn jumpsd_fillhisto(data: *mut c_char, len: c_int) {
    if data.is_null() || len <= 0 {
        return;
    }
    // SAFETY: `data`/`len` point at `len * sizeof(int)` valid bytes of the
    // DMA ring buffer for the duration of this call, per wrapped_jumpsd_dma's
    // contract (the same buffer `libjumpsd.so`'s original C callback used).
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, len as usize * 4) };
    let mut guard = SHARED.lock().unwrap();
    let Some(shared) = guard.as_mut() else { return };
    let _ = shared.dump.write(bytes);
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_ne_bytes(c.try_into().expect("chunks_exact(4)")))
        .collect();
    let events = shared.decoder.feed(&words);
    if !events.is_empty() {
        let _ = shared.sender.send(events);
    }
}

#[derive(HasParams)]
#[params(kind = "input", type = "jumiom")]
pub struct JumiomInput {
    name: ModuleId,
    device: i32,
    #[param(has_setter = true, datatype = "tof1|raw|ramp",
            help = "Acquisition mode; applied to hardware and decoding at the next Start")]
    mode: JumiomMode,
    #[param(has_setter = true, datatype = "JumiomCalibration or null",
            help = "Hardware calibration values, applied to the device at the next Start")]
    calibration: Option<JumiomCalibration>,
    receiver: Receiver<Vec<Event>>,
    thread: Option<JoinHandle<()>>,
}

pub fn start(config: JumiomConfig, _confdir: &Path, common: InputCommon) -> UResult<()> {
    let (sender, receiver) = crate::channel::unbounded();
    {
        let mut guard = SHARED.lock().unwrap();
        if guard.is_some() {
            return Err(UError::Other(anyhow!(
                "Only one Jumiom input can be active per umami process")));
        }
        *guard = Some(Shared {
            mode: config.mode,
            calibration: config.calibration,
            decoder: decode::JumiomDecoder::new(config.mode),
            dump: DumpHandler::default(),
            sender,
        });
    }
    let input = JumiomInput {
        name: common.name,
        device: config.device,
        mode: config.mode,
        calibration: config.calibration,
        receiver,
        thread: None,
    };
    input.start_main_loop(common)?;
    Ok(())
}

impl JumiomInput {
    fn set_mode(&mut self, mode: JumiomMode) -> UResult<()> {
        self.mode = mode;
        if let Some(shared) = SHARED.lock().unwrap().as_mut() {
            shared.mode = mode;
        }
        Ok(())
    }

    fn set_calibration(&mut self, calibration: Option<JumiomCalibration>) -> UResult<()> {
        self.calibration = calibration;
        if let Some(shared) = SHARED.lock().unwrap().as_mut() {
            shared.calibration = calibration;
        }
        Ok(())
    }
}

impl Input for JumiomInput {
    fn description(&self) -> String {
        format!("Jumiom {} device {} ({:?})", self.name, self.device, self.mode)
    }

    fn handle(&mut self, cmd: Command) -> UResult<CommandReply> {
        if let Command::SetRawDump { enable, path } = cmd
            && let Some(shared) = SHARED.lock().unwrap().as_mut()
        {
            shared.dump.configure(enable, path)?;
        }
        Ok(CommandReply::Ok)
    }

    fn start(&mut self, run_id: String) -> UResult<()> {
        {
            let mut guard = SHARED.lock().unwrap();
            let shared = guard.as_mut().expect("Jumiom shared state installed at construction");
            shared.decoder = decode::JumiomDecoder::new(self.mode);
            shared.dump.start(self.name, &run_id)?;
        }
        let device = self.device;
        let name = self.name;
        let handle = thread::Builder::new()
            .name("Jumiom DMA".into())
            .spawn(move || {
                // SAFETY: wrapped_jumpsd_dma is the whole DMA setup/poll/teardown
                // loop from DriverJumiom's libjumpsd.so; it calls back into
                // jumpsd_fillhisto/jumpsd_setup_callback above.
                let rc = unsafe { wrapped_jumpsd_dma(device) };
                if rc != 0 {
                    lprintln!(ERROR, [name] "Jumiom DMA loop for device {device} exited with code {rc}");
                }
            })
            .context("Spawning Jumiom DMA thread")?;
        self.thread = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> UResult<()> {
        let fd = FD.load(Ordering::SeqCst);
        if fd >= 0 {
            // SAFETY: fd was obtained from jumpsd_setup_callback and is only
            // ever used to toggle a driver-side status bit, valid for as long
            // as the DMA thread (which owns the fd) may still be running.
            unsafe { write_status(fd, DMA_BREAK); }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        if let Some(shared) = SHARED.lock().unwrap().as_mut() {
            shared.dump.stop();
        }
        Ok(())
    }

    fn reset(&mut self) -> UResult<()> {
        self.stop()
    }

    fn read_events(&mut self) -> UResult<Vec<Event>> {
        let mut events = match self.receiver.recv_timeout(Duration::from_millis(300)) {
            Ok(chunk) => chunk,
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(UError::Other(anyhow!(
                    "Jumiom DMA channel for device {} disconnected unexpectedly", self.device)));
            }
        };
        while let Ok(mut chunk) = self.receiver.try_recv() {
            events.append(&mut chunk);
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamMap;

    fn make_input() -> JumiomInput {
        let (_sender, receiver) = crate::channel::unbounded::<Vec<Event>>();
        JumiomInput {
            name: ModuleId::new("jumiom".into()),
            device: 0,
            mode: JumiomMode::Raw,
            calibration: None,
            receiver,
            thread: None,
        }
    }

    #[test]
    fn test_get_params_reports_mode_and_calibration() {
        let input = make_input();
        let params = input.get_params(false).unwrap();
        assert_eq!(params["mode"]["value"], "raw");
        assert!(params["calibration"]["value"].is_null());
    }

    #[test]
    fn test_update_params_stages_mode_and_calibration_for_next_start() {
        let mut input = make_input();
        let mut set = ParamMap::new();
        set.insert("mode".into(), serde_json::json!("tof1"));
        set.insert("calibration".into(), serde_json::json!({
            "thresholds": [1, 2, 3],
            "poti": [4, 5, 6, 7],
            "dac1": [0, 0, 0, 0],
            "dac2": [0, 0, 0, 0],
            "pileup": 8,
            "monitor_delay": 9,
            "chopper_delay": 10,
        }));
        // no hardware is touched here; the new values only take effect via
        // jumpsd_setup_callback, next time this input's Start is handled
        input.update_params(ModuleId::new("jumiom".into()), set).unwrap();

        assert_eq!(input.mode, JumiomMode::Tof1);
        assert_eq!(input.calibration.unwrap().pileup, 8);
    }
}
