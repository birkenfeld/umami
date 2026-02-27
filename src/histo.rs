// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

pub struct Histogram {
    data: Vec<u32>,
    nx: usize,
    ny: usize
}

impl Histogram {
    pub fn new(nx: usize, ny: usize) -> Self {
        Self {
            data: vec![0; nx * ny],
            nx,
            ny
        }
    }

    pub fn add(&mut self, x: usize, y: usize) {
        if x < self.nx && y < self.ny {
            self.data[y * self.nx + x] += 1;
        }
    }
}
