// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use anyhow::{anyhow, Context};
use nix::{fcntl::OFlag, unistd::ftruncate};
use nix::sys::{mman::{shm_open, mmap, MapFlags, ProtFlags}, stat::Mode};
use crate::config::HistoConfig;
use crate::error::UResult;
use crate::input::InputState;

pub const MAX_MODULES: usize = 128;
pub const MAX_HISTO_SIZE: usize = 1024 * 1024 * 1024;  // 1 GB shmem

pub struct ShmBox {
    ptr: NonNull<ShmInterface>,
    max_nt: u32,
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
        let size = (self.nx * self.ny * self.max_nt) as usize;
        unsafe {
            let histo_ptr = self.ptr.as_ptr().add(1) as *mut u32;
            std::slice::from_raw_parts_mut(histo_ptr, size).fill(0);
        };
    }

    pub fn add_histo(&mut self, x: u32, y: u32, mut t: u32) {
        if self.nt > 0 && t >= self.nt {
            return;
        }
        if self.nt == 0 {
            t = 0;
        }
        if x < self.nx && y < self.ny {
            let off = t * (self.nx * self.ny) + y * self.nx + x;
            unsafe {
                let histo_ptr = self.ptr.as_ptr().add(1) as *mut u32;
                let place_ptr = histo_ptr.add(off as usize);
                ptr::write(place_ptr, place_ptr.read() + 1);
            }
        }
    }
}

#[repr(C)]
// This trait impl is not actually used but ensures that initializing
// the SHM does not create undefined behavior.
#[derive(zerocopy::FromBytes)]
pub struct ShmInterface {
    pub run_id: [u8; 128],
    pub state: [u8; MAX_MODULES],
    pub modules: u32,
    pub nx: u32,
    pub ny: u32,
    pub nt: u32,
}

impl ShmInterface {
    pub fn set_state(&mut self, state: crate::input::InputState) {
        match state {
            InputState::Stopped(mid) => self.state[mid.0 as usize] = 0,
            InputState::Running(mid) => self.state[mid.0 as usize] = 1,
            InputState::Ended(mid) => self.state[mid.0 as usize] = 2,
            InputState::Errored(mid) => self.state[mid.0 as usize] = 3,
        }
    }

    pub fn set_run_id(&mut self, run_id: &str) {
        let bytes = run_id.as_bytes();
        let len = bytes.len().min(self.run_id.len() - 1);
        self.run_id[..len].copy_from_slice(&bytes[..len]);
        self.run_id[len..128].fill(0);
    }

    pub fn create(name: &str, config: &HistoConfig, modules: usize) -> UResult<ShmBox> {
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
        let mut shmbox = ShmBox { ptr: ptr.cast(), max_nt: config.max_nt as u32 };
        shmbox.run_id.fill(0);
        shmbox.state.fill(0);
        shmbox.modules = modules as u32;
        shmbox.nx = config.nx as u32;
        shmbox.ny = config.ny as u32;
        shmbox.nt = 0;
        Ok(shmbox)
    }

}
