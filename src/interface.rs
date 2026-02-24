// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use anyhow::Context;
use shmem_bind::{ShmemBox, self as shmem};
use crate::error::UResult;

#[repr(C)]
#[derive(Copy, Clone)]
// This trait impl is not actually used but ensures that initializing
// the SHM does not create undefined behavior.
#[derive(zerocopy::FromBytes)]
pub struct ShmInterface {
    pub state: u32,
}

// pub type MappedShmInterface = shmem::Map<ShmInterface>;

impl ShmInterface {
    pub fn map(name: &str) -> UResult<ShmemBox<ShmInterface>> {
        let shared_mem = shmem::Builder::new(name)
            .with_size(std::mem::size_of::<ShmInterface>() as i64)
            .open()
            .context("Failed to map shared memory")?;
        Ok(unsafe { shared_mem.boxed::<ShmInterface>() })
    }

    pub fn initialize(&mut self) {
        self.state = 0;
    }
}
