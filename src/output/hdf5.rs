// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::path::PathBuf;

use anyhow::{anyhow, Context};
use crate::event::{Event, EventData};
use crate::error::UResult;
use crate::params::HasParams;
use super::{Output, OutputCommon};

// TODO:
// - behind feature?
// - filename template
/// Output for a HDF5-File containing events following the NXevent_data format.
///
///  Currently the following fields are not supported:
///  "event_time_zero"
///  "event_index"
///  "cue_timestamp_zero"
///  "cue_index"
///  "pulse_height"
///
#[derive(HasParams)]
pub struct HDF5EventsOutput {
    #[param(help="Directory to write hdf5 event files to.")]
    dir: PathBuf,
    #[param(help="Name of file to create within dir, defaults to <run number>.h5", datatype="null or String")]
    filename: Option<String>,
    file: Option<hdf5::File>,
    id_buffer: Vec<u32>,
    offset_buffer: Vec<f64>,
}

impl HDF5EventsOutput {
    const BUFFER_SIZE: usize = 8192;

    fn map_to_index(x: u32, y: u32) -> u32 {
        1024 * y + x
    }

    fn write_chunk(&mut self) -> UResult<()> {
        if let Some(file) = &self.file {
            let event_id = file.dataset("event_id")
                               .context("Getting event id dataset")?;
            let event_offset = file.dataset("event_time_offset")
                                   .context("Getting event time offset dataset")?;
            let cur_size = event_id.size();
            let new_size = cur_size + self.id_buffer.len();
            event_id.resize(new_size).context("Resizing event id dataset")?;
            event_id.write_slice(&self.id_buffer, cur_size..new_size)
                    .context("Writing event id dataset")?;
            event_offset.resize(new_size).context("Resizing event time offset dataset")?;
            event_offset.write_slice(&self.offset_buffer, cur_size..new_size)
                        .context("Writing event time offset dataset")?;
        }
        self.id_buffer.clear();
        self.offset_buffer.clear();
        Ok(())
    }
}

impl Output for HDF5EventsOutput {
    fn from_config(_: &OutputCommon, config: toml::Table) -> UResult<Self> where Self: Sized {
        let dir = config.get("dir")
            .ok_or_else(|| anyhow!("Missing 'dir' in file output config"))?
            .as_str()
            .ok_or_else(|| anyhow!("'dir' in file output config must be a string"))?;
        Ok(HDF5EventsOutput {
            dir: PathBuf::from(dir),
            filename: None,
            file: None,
            id_buffer: Vec::with_capacity(2 * Self::BUFFER_SIZE),
            offset_buffer: Vec::with_capacity(2 * Self::BUFFER_SIZE),
        })
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        let path = match self.filename.as_deref() {
            Some(name) => self.dir.join(name),
            None => self.dir.join(format!("{run}.h5")),
        };
        let file = hdf5::File::create(&path)
            .with_context(|| format!("Creating HDF5 output file at {}.", path.display()))?;
        let _ = file
            .new_dataset::<f64>()
            .shape(hdf5::Extent::resizable(0))
            .create("event_time_offset")
            .context("Creating time offset dataset")?;
        let _ = file
            .new_dataset::<u32>()
            .shape(hdf5::Extent::resizable(0))
            .create("event_id")
            .context("Creating event id dataset")?;
        self.file = Some(file);
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        self.write_chunk()?;
        self.file = None;
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        for event in events {
            match event.data {
                // TODO: zero timestamps handling (chopper?)
                EventData::Neutron => {
                    self.id_buffer.push(HDF5EventsOutput::map_to_index(event.x, event.y));
                    self.offset_buffer.push(event.rel_time.into());
                },
                _ => (),
            }
        }
        if self.id_buffer.len() >= Self::BUFFER_SIZE {
            self.write_chunk()?;
        }
        Ok(())
    }
}
