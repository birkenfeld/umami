// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use anyhow::{anyhow, Context};
use nix::{fcntl::OFlag, unistd::ftruncate};
use nix::sys::{mman::{shm_open, mmap, MapFlags, ProtFlags}, stat::Mode};
use crate::config::HistoConfig;
use crate::error::UResult;

pub const MAX_MODULES: usize = 128;
pub const MAX_HISTO_SIZE: usize = 1024 * 1024 * 1024;  // 1 GB shmem

pub struct ShmBox {
    ptr: NonNull<ShmInterface>,
}

unsafe impl Sync for ShmBox {}
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
        let histo_ptr = unsafe { self.ptr.as_ptr().add(1) as *mut u32 };
        let histo_slice = unsafe {
            std::slice::from_raw_parts_mut(histo_ptr, (self.nx * self.ny * self.nt) as usize)
        };
        histo_slice.fill(0);
    }

    pub fn add_histo(&mut self, x: u32, y: u32, t: u32) {
        if x < self.nx && y < self.ny && t < self.nt {
            let histo_ptr = unsafe { self.ptr.as_ptr().add(1) as *mut u32 };
            let histo_slice = unsafe {
                std::slice::from_raw_parts_mut(histo_ptr, (self.nx * self.ny * self.nt) as usize)
            };
            let idx = (t * (self.nx * self.ny) + y * self.nx + x) as usize;
            histo_slice[idx] += 1;
        }
    }
}

#[repr(C)]
// This trait impl is not actually used but ensures that initializing
// the SHM does not create undefined behavior.
#[derive(zerocopy::FromBytes)]
pub struct ShmInterface {
    pub state: [u8; MAX_MODULES],
    pub modules: u32,
    pub nx: u32,
    pub ny: u32,
    pub nt: u32,
}

impl ShmInterface {
    pub fn reset(&mut self, n: u32) {
        self.state.fill(0);
        self.modules = n;
        self.nx = 0;
        self.ny = 0;
        self.nt = 0;
    }

    pub fn create(name: &str, config: &HistoConfig) -> UResult<ShmBox> {
        let max_size = config.nx * config.ny * config.max_nt;
        if max_size == 0 {
            Err(anyhow!("Requested histogram size is zero"))?;
        }
        if max_size > MAX_HISTO_SIZE {
            Err(anyhow!("Requested histogram size {} exceeds maximum of {}",
                        max_size, MAX_HISTO_SIZE))?;
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
        let shmbox = ShmBox { ptr: ptr.cast() };
        Ok(shmbox)
    }

}
