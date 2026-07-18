// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fs::File;
use std::io::{Write, BufWriter};
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use anyhow::{anyhow, Context};
use nix::{fcntl::OFlag, unistd::ftruncate};
use nix::sys::{mman::{shm_open, mmap, MapFlags, ProtFlags},
               stat::{fstat, Mode}};
use crate::config::HistoConfig;
use crate::error::UResult;

pub const MAX_INPUTS: usize = 128;
pub const MAX_HISTO_SIZE: usize = 1024 * 1024 * 1024;  // 1 GB shmem

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
    pub fn clear_histo(&mut self) {
        let size = (self.nx * self.ny * self.nt) as usize;
        unsafe {
            let histo_ptr = self.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts_mut(histo_ptr, size).fill(0);
        };
    }

    pub fn add_histo(&mut self, x: u32, y: u32, t: u32) {
        if x < self.nx && y < self.ny && t < self.nt {
            let off = t * (self.nx * self.ny) + y * self.nx + x;
            unsafe {
                let histo_ptr = self.ptr.as_ptr().add(1).cast::<u32>();
                let place_ptr = histo_ptr.add(off as usize);
                ptr::write(place_ptr, place_ptr.read() + 1);
            }
        }
    }

    pub fn histo_total(&self) -> u64 {
        let size = (self.nx * self.ny * self.nt) as usize;
        let histo = unsafe {
            let ptr = self.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, size)
        };
        histo.iter().map(|&v| v as u64).sum()
    }

    pub fn histo_value(&self, x: u32, y: u32, t: u32) -> u32 {
        if x < self.nx && y < self.ny && t < self.nt {
            let off = t * (self.nx * self.ny) + y * self.nx + x;
            unsafe {
                let ptr = self.ptr.as_ptr().add(1).cast::<u32>().add(off as usize);
                ptr.read()
            }
        } else {
            0
        }
    }

    pub fn save_to_file(&self, filename: &str, max_nt: usize) -> UResult<()> {
        let nt = self.nt.min(max_nt as u32);
        let size = (self.nx * self.ny * nt) as usize;
        let histo_slice = unsafe {
            let histo_ptr = self.ptr.as_ptr().add(1) as *const u32;
            std::slice::from_raw_parts(histo_ptr, size)
        };
        let file = File::create(filename)
            .context("Creating histogram output file")?;
        let mut writer = BufWriter::new(file);
        for t in 0..nt {
            for y in 0..self.ny {
                for x in 0..self.nx {
                    let off = t * (self.nx * self.ny) + y * self.nx + x;
                    let count = histo_slice[off as usize];
                    write!(&mut writer, " {count}")
                        .context("Writing histogram data to file")?;
                }
                writeln!(&mut writer).context("Writing histogram data to file")?;
            }
            writeln!(&mut writer, "\n").context("Writing histogram data to file")?;
        }
        Ok(())
    }
}

#[repr(C)]
// This trait impl is not actually used but ensures that initializing
// the SHM does not create undefined behavior.
#[derive(zerocopy::FromBytes)]
pub struct ShmInterface {
    pub run_id: [u8; 128],
    pub global_state: u32,
    pub nx: u32,
    pub ny: u32,
    pub nt: u32,
}

impl ShmInterface {
    pub fn set_run_id(&mut self, run_id: &str) {
        let bytes = run_id.as_bytes();
        let len = bytes.len().min(self.run_id.len() - 1);
        self.run_id[..len].copy_from_slice(&bytes[..len]);
        self.run_id[len..128].fill(0);
    }

    pub fn set_initialized(&mut self) {
        self.global_state |= 1;
    }

    pub fn create(name: &str, config: &HistoConfig) -> UResult<ShmBox> {
        let max_size = config.nx * config.ny * config.max_nt;
        if max_size == 0 {
            Err(anyhow!("Requested histogram size is zero"))?;
        }
        if max_size > MAX_HISTO_SIZE {
            Err(anyhow!("Requested histogram size {max_size} exceeds maximum of {MAX_HISTO_SIZE}"))?;
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
        shmbox.run_id.fill(0);
        shmbox.global_state = 0;
        shmbox.nx = config.nx as u32;
        shmbox.ny = config.ny as u32;
        shmbox.nt = config.max_nt as u32;
        Ok(shmbox)
    }

    pub fn open(name: &str) -> UResult<ShmBox> {
        let fd = shm_open(name, OFlag::O_RDONLY, Mode::empty())
            .context("Opening shared memory block")?;
        let total_size = fstat(&fd).context("Stat shared memory block")?.st_size as usize;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SHM_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_shm_name() -> String {
        let id = SHM_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("umami_test_{id}_{}", std::process::id())
    }

    fn test_config() -> HistoConfig {
        HistoConfig { nx: 4, ny: 4, max_nt: 4 }
    }

    #[test]
    fn test_shm_create_and_basic_ops() {
        let name = unique_shm_name();
        let mut shm = ShmInterface::create(&name, &test_config()).unwrap();

        // set_run_id
        shm.set_run_id("run_001");
        let run_id = std::str::from_utf8(&shm.run_id).unwrap();
        assert!(run_id.starts_with("run_001"));

        // set_initialized
        assert_eq!(shm.global_state & 1, 0);
        shm.set_initialized();
        assert_eq!(shm.global_state & 1, 1);

        // add_histo in bounds
        shm.add_histo(0, 0, 0);
        shm.add_histo(0, 0, 0);
        shm.add_histo(1, 2, 3);

        // verify histogram values
        let histo = unsafe {
            let ptr = shm.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, 4 * 4 * 4)
        };
        // bin (0,0,0) → offset 0*16 + 0*4 + 0 = 0
        assert_eq!(histo[0], 2);
        // bin (1,2,3) → offset 3*16 + 2*4 + 1 = 57
        assert_eq!(histo[57], 1);

        // cleanup
        nix::sys::mman::shm_unlink(name.as_bytes()).ok();
    }

    #[test]
    fn test_shm_add_out_of_bounds_ignored() {
        let name = unique_shm_name();
        let mut shm = ShmInterface::create(&name, &test_config()).unwrap();
        shm.add_histo(10, 10, 10); // all out of bounds
        // should not panic
        nix::sys::mman::shm_unlink(name.as_bytes()).ok();
    }

    #[test]
    fn test_shm_clear_histo() {
        let name = unique_shm_name();
        let mut shm = ShmInterface::create(&name, &test_config()).unwrap();
        shm.add_histo(0, 0, 0);
        shm.add_histo(1, 1, 1);
        shm.clear_histo();
        let histo = unsafe {
            let ptr = shm.ptr.as_ptr().add(1).cast::<u32>();
            std::slice::from_raw_parts(ptr, 4 * 4 * 4)
        };
        assert!(histo.iter().all(|&v| v == 0));
        nix::sys::mman::shm_unlink(name.as_bytes()).ok();
    }

    #[test]
    fn test_shm_save_to_file() {
        let name = unique_shm_name();
        let mut shm = ShmInterface::create(&name, &test_config()).unwrap();
        shm.add_histo(0, 0, 0);
        shm.add_histo(0, 0, 0);
        let path = format!("/tmp/umami_test_histo_{}", std::process::id());
        shm.save_to_file(&path, 4).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("2")); // two counts at (0,0,0)
        std::fs::remove_file(&path).ok();
        nix::sys::mman::shm_unlink(name.as_bytes()).ok();
    }

    #[test]
    fn test_shm_zero_size_fails() {
        let name = unique_shm_name();
        let config = HistoConfig { nx: 0, ny: 1, max_nt: 1 };
        let result = ShmInterface::create(&name, &config);
        assert!(result.is_err());
    }
}
