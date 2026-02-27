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

    pub fn plot(&self) {
        use plotters::prelude::*;
        use plotters::style::colors::colormaps::ViridisRGB;

        let max = (self.data.iter().cloned().max().unwrap_or(1) as f64).log10();

        const OUT_FILE_NAME: &str = "histo.png";
        let root = BitMapBackend::new(OUT_FILE_NAME, (1600, 1200)).into_drawing_area();

        root.fill(&WHITE).unwrap();

        let mut chart = ChartBuilder::on(&root)
            .margin(20)
            .x_label_area_size(10)
            .y_label_area_size(10)
            .build_cartesian_2d(0..self.nx, 0..self.ny).unwrap();

        chart
            .configure_mesh()
            .disable_x_mesh()
            .disable_y_mesh()
            .draw().unwrap();

        chart.draw_series(
            (0..self.nx).flat_map(move |x| (0..self.ny).map(move |y| {
                Rectangle::new(
                    [(x, y), (x + 1, y + 1)],
                    ViridisRGB::get_color(
                        (self.data[y * self.nx + x] as f64 + 0.1).log10() / max
                    ).filled()
                )
            }))).unwrap();

        root.present().expect("no plot");
    }
}
