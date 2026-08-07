// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fs::File;
use std::io::{Write, BufWriter};
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use anyhow::{anyhow, Context};
use nix::{fcntl::OFlag, unistd::ftruncate};
use nix::sys::{mman::{shm_open, mmap, MapFlags, ProtFlags}, stat::Mode};
use crate::config::HistoConfig;
use crate::error::UResult;
use crate::event::{EventHisto, EventTime};

/// Maximum number of histogram bins (nx * ny * nt) that can be allocated in
/// shared memory.  This is a sanity check.
pub const MAX_HISTO_SIZE: usize = 1024 * 1024 * 1024;  // 4 GB shmem

/// "UMAMI" + a two-digit ASCII version, lets a reader detect a layout version
/// mismatch by comparing against `SHM_MAGIC`. When changing this, adapt client
/// code too.
pub const SHM_MAGIC: [u8; 8] = *b"UMAMI01 ";

/// Size of the `ShmInterface` header.  Changing this must change SHM_MAGIC.
#[allow(unused)]
const SHM_HEADER_SIZE: usize = 224;

/// Number of `Monitor {num}` counter slots reserved in the header (indices
/// 0..MONITOR_COUNTERS); a `num` outside this range is silently ignored.
const MONITOR_COUNTERS: usize = 5;

/// `global_state` bit set while a run is active (between `StartOfRun` and
/// `EndOfRun`) -- lets a client freeze its `run_start`-derived elapsed-time
/// display once a run ends, instead of counting up forever.
pub const RUNNING_BIT: u32 = 1 << 1;

pub struct ShmBox {
    ptr: NonNull<ShmInterface>,
}

unsafe impl Send for ShmBox {}

impl Deref for ShmBox {
    type Target = ShmInterface;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl DerefMut for ShmBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl ShmBox {
    fn histo_size(&self, nt: u16) -> usize {
        self.nx as usize * self.ny as usize * nt as usize
    }

    pub fn clear_histo(&mut self) {
        let size = self.histo_size(self.nt);
        unsafe {
            let histo_ptr = self.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts_mut(histo_ptr, size).fill(0);
        };
    }

    pub fn add_histo(&mut self, h: EventHisto) {
        // TODO: i
        if h.x < self.nx && h.y < self.ny && h.t < self.nt {
            let off = h.t as usize * self.nx as usize * self.ny as usize
                    + h.y as usize * self.nx as usize
                    + h.x as usize;
            unsafe {
                let histo_ptr = self.ptr.as_ptr().add(1).cast::<u32>();
                let place_ptr = histo_ptr.add(off);
                ptr::write(place_ptr, place_ptr.read() + 1);
            }
            self.total_neutrons += 1;
        }
    }

    #[cfg(test)]
    pub fn histo_data(&self) -> Vec<u32> {
        let size = self.histo_size(self.nt);
        let histo = unsafe {
            let ptr = self.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, size)
        };
        histo.to_vec()
    }

    pub fn save_to_file(&self, filename: &str, max_nt: usize) -> UResult<()> {
        let file = File::create(filename)
            .context("Creating histogram output file")?;
        self.write_histo(BufWriter::new(file), max_nt)
    }

    fn write_histo(&self, mut writer: impl Write, max_nt: usize) -> UResult<()> {
        let nt = self.nt.min(max_nt as u16);
        let size = self.histo_size(nt);
        let histo_slice = unsafe {
            let histo_ptr = self.ptr.as_ptr().add(1) as *const u32;
            std::slice::from_raw_parts(histo_ptr, size)
        };
        for t in 0..nt {
            for y in 0..self.ny {
                for x in 0..self.nx {
                    let off = t as usize * self.nx as usize * self.ny as usize
                            + y as usize * self.nx as usize
                            + x as usize;
                    let count = histo_slice[off];
                    write!(writer, " {count}")
                        .context("Writing histogram data to file")?;
                }
                writeln!(writer).context("Writing histogram data to file")?;
            }
            writeln!(writer, "\n").context("Writing histogram data to file")?;
        }
        Ok(())
    }
}

#[repr(C)]
// This trait impl is not actually used but ensures that initializing
// the SHM does not create undefined behavior.
#[derive(zerocopy::FromBytes)]
pub struct ShmInterface {
    pub magic: [u8; 8],
    pub run_id: [u8; 128],
    pub global_state: u32,
    pub nx: u16,
    pub ny: u16,
    pub nt: u16,
    pub ni: u16,
    pub run_start: u32,
    pub total_events: u64,
    pub total_neutrons: u64,
    pub lifetime_ns: i64,
    pub tzero_count: u64,
    pub monitor_counts: [u64; MONITOR_COUNTERS],
}

impl ShmInterface {
    pub fn set_run_id(&mut self, run_id: &str) {
        let bytes = run_id.as_bytes();
        let len = bytes.len().min(self.run_id.len() - 1);
        self.run_id[..len].copy_from_slice(&bytes[..len]);
        self.run_id[len..128].fill(0);
    }

    pub fn set_run_start(&mut self, unix_secs: u32) {
        self.run_start = unix_secs;
    }

    pub fn set_initialized(&mut self) {
        self.global_state |= 1;
    }

    pub fn set_running(&mut self, running: bool) {
        if running {
            self.global_state |= RUNNING_BIT;
        } else {
            self.global_state &= !RUNNING_BIT;
        }
    }

    pub fn add_events(&mut self, n: usize) {
        self.total_events += n as u64;
    }

    pub fn add_tzero(&mut self) {
        self.tzero_count += 1;
    }

    pub fn add_monitor(&mut self, num: u8) {
        if let Some(count) = self.monitor_counts.get_mut(num as usize) {
            *count += 1;
        }
    }

    pub fn set_lifetime(&mut self, ns: EventTime) {
        self.lifetime_ns = ns.0;
    }

    pub fn clear_counters(&mut self) {
        self.total_events = 0;
        self.total_neutrons = 0;
        self.lifetime_ns = 0;
        self.tzero_count = 0;
        self.monitor_counts = [0; MONITOR_COUNTERS];
    }

    pub fn create(name: &str, config: &HistoConfig) -> UResult<ShmBox> {
        let max_size = config.nx * config.ny * config.max_nt;
        if max_size == 0 {
            Err(anyhow!("Requested histogram size is zero"))?;
        }
        if max_size > MAX_HISTO_SIZE {
            Err(anyhow!("Requested histogram size {max_size} exceeds maximum of \
                         {MAX_HISTO_SIZE} bins"))?;
        }

        let total_size = size_of::<ShmInterface>() + max_size * size_of::<u32>();
        let fd = shm_open(name, OFlag::O_CREAT | OFlag::O_RDWR, Mode::S_IRUSR | Mode::S_IWUSR)
            .context("Creating shared memory block")?;
        ftruncate(&fd, total_size as i64)
            .context("Setting size of shared memory block")?;
        let ptr = unsafe {
            mmap(None, NonZeroUsize::new(total_size).expect("size"),
                 ProtFlags::PROT_WRITE, MapFlags::MAP_SHARED, fd, 0)
                .context("Mapping shared memory block")?
        };
        let mut shmbox = ShmBox { ptr: ptr.cast() };
        shmbox.magic = SHM_MAGIC;
        shmbox.run_id.fill(0);
        shmbox.global_state = 0;
        shmbox.nx = config.nx as u16;
        shmbox.ny = config.ny as u16;
        shmbox.nt = config.max_nt as u16;
        shmbox.ni = config.max_ni as u16;
        shmbox.run_start = 0;
        shmbox.total_events = 0;
        shmbox.total_neutrons = 0;
        shmbox.lifetime_ns = 0;
        shmbox.tzero_count = 0;
        shmbox.monitor_counts = [0; MONITOR_COUNTERS];
        Ok(shmbox)
    }

    #[cfg(test)]
    pub fn open(name: &str) -> UResult<ShmBox> {
        let fd = shm_open(name, OFlag::O_RDONLY, Mode::empty())
            .context("Opening shared memory block")?;
        let total_size = nix::sys::stat::fstat(&fd).context("Stat shared memory block")?.st_size as usize;
        if total_size < size_of::<ShmInterface>() {
            Err(anyhow!("Shared memory block too small for header"))?;
        }
        let ptr = unsafe {
            mmap(None, NonZeroUsize::new(total_size).expect("size"),
                 ProtFlags::PROT_READ, MapFlags::MAP_SHARED, fd, 0)
                .context("Mapping shared memory block for reading")?
        };
        Ok(ShmBox { ptr: ptr.cast() })
    }
}

/// Owns a unique test shm segment name and unlinks it on drop, so a panicking
/// test still cleans up instead of leaking the segment.
#[cfg(test)]
pub(crate) struct ShmGuard(String);

#[cfg(test)]
impl ShmGuard {
    pub fn unique() -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(format!("umami_test_{id}_{}", std::process::id()))
    }

    /// Wraps a name decided elsewhere (e.g. derived from a config's ipc_name)
    /// so it still gets unlinked on drop.
    pub fn for_name(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl Drop for ShmGuard {
    fn drop(&mut self) {
        nix::sys::mman::shm_unlink(self.0.as_bytes()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> HistoConfig {
        HistoConfig { nx: 4, ny: 4, max_nt: 4, max_ni: 0 }
    }

    #[test]
    fn test_shm_interface_size() {
        // pins the header size, so a future field addition/reordering can't
        // silently shift the histogram's starting offset unnoticed
        assert_eq!(size_of::<ShmInterface>(), SHM_HEADER_SIZE);
    }

    #[test]
    fn test_shm_create_and_basic_ops() {
        let shm_guard = ShmGuard::unique();
        let mut shm = ShmInterface::create(shm_guard.name(), &test_config()).unwrap();

        // create() stamps the magic value
        assert_eq!(shm.magic, SHM_MAGIC);

        // set_run_id
        shm.set_run_id("run_001");
        let run_id = std::str::from_utf8(&shm.run_id).unwrap();
        assert!(run_id.starts_with("run_001"));

        // set_initialized
        assert_eq!(shm.global_state & 1, 0);
        shm.set_initialized();
        assert_eq!(shm.global_state & 1, 1);

        // add_histo in bounds
        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 });
        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 });
        shm.add_histo(EventHisto { x: 1, y: 2, t: 3, i: 0 });

        // verify histogram values
        let histo = unsafe {
            let ptr = shm.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, 4 * 4 * 4)
        };
        // bin (0,0,0) → offset 0*16 + 0*4 + 0 = 0
        assert_eq!(histo[0], 2);
        // bin (1,2,3) → offset 3*16 + 2*4 + 1 = 57
        assert_eq!(histo[57], 1);
    }

    #[test]
    fn test_shm_counters() {
        let shm_guard = ShmGuard::unique();
        let mut shm = ShmInterface::create(shm_guard.name(), &test_config()).unwrap();

        shm.add_events(3);
        shm.add_events(2);
        assert_eq!(shm.total_events, 5);

        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 }); // in bounds
        shm.add_histo(EventHisto { x: 10, y: 10, t: 10, i: 0 }); // out of bounds
        assert_eq!(shm.total_neutrons, 1);

        shm.add_tzero();
        shm.add_tzero();
        assert_eq!(shm.tzero_count, 2);

        shm.add_monitor(0);
        shm.add_monitor(4);
        shm.add_monitor(4);
        shm.add_monitor(100); // out of range, silently ignored
        assert_eq!(shm.monitor_counts, [1, 0, 0, 0, 2]);

        shm.set_lifetime(EventTime(12345));
        assert_eq!(shm.lifetime_ns, 12345);

        shm.clear_counters();
        assert_eq!(shm.total_events, 0);
        assert_eq!(shm.total_neutrons, 0);
        assert_eq!(shm.lifetime_ns, 0);
        assert_eq!(shm.tzero_count, 0);
        assert_eq!(shm.monitor_counts, [0; 5]);
    }

    #[test]
    fn test_shm_add_out_of_bounds_ignored() {
        let shm_guard = ShmGuard::unique();
        let mut shm = ShmInterface::create(shm_guard.name(), &test_config()).unwrap();
        shm.add_histo(EventHisto { x: 10, y: 10, t: 10, i: 0 }); // all out of bounds
        // should not panic
    }

    #[test]
    fn test_shm_clear_histo() {
        let shm_guard = ShmGuard::unique();
        let mut shm = ShmInterface::create(shm_guard.name(), &test_config()).unwrap();
        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 });
        shm.add_histo(EventHisto { x: 1, y: 1, t: 1, i: 0 });
        shm.clear_histo();
        let histo = unsafe {
            let ptr = shm.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, 4 * 4 * 4)
        };
        assert!(histo.iter().all(|&v| v == 0));
    }

    #[test]
    fn test_shm_save_to_file() {
        let shm_guard = ShmGuard::unique();
        let mut shm = ShmInterface::create(shm_guard.name(), &test_config()).unwrap();
        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 });
        shm.add_histo(EventHisto { x: 0, y: 0, t: 0, i: 0 });
        let path = format!("/tmp/umami_test_histo_{}", std::process::id());
        shm.save_to_file(&path, 4).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("2")); // two counts at (0,0,0)
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shm_save_to_file_large_offsets() {
        let shm_guard = ShmGuard::unique();
        // nx*ny*nt exceeds u16::MAX, so offsets must be computed in usize
        let config = HistoConfig { nx: 56, ny: 1024, max_nt: 2, max_ni: 0 };
        let mut shm = ShmInterface::create(shm_guard.name(), &config).unwrap();
        shm.add_histo(EventHisto { x: 10, y: 1000, t: 1, i: 0 });
        let mut buf = Vec::new();
        shm.write_histo(&mut buf, 2).unwrap();
        let content = String::from_utf8(buf).unwrap();
        assert!(content.contains('1'));
    }

    #[test]
    fn test_shm_zero_size_fails() {
        let shm_guard = ShmGuard::unique();
        let config = HistoConfig { nx: 0, ny: 1, max_nt: 1, max_ni: 0 };
        let result = ShmInterface::create(shm_guard.name(), &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_shm_oversized_fails() {
        let shm_guard = ShmGuard::unique();
        // exceeds MAX_HISTO_SIZE without ever attempting to allocate it
        let config = HistoConfig { nx: 1_000_000, ny: 1_000_000, max_nt: 1_000, max_ni: 0 };
        let result = ShmInterface::create(shm_guard.name(), &config);
        assert!(result.is_err());
    }
}
