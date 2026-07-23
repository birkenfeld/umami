// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::path::PathBuf;

use anyhow::{anyhow, Context};
use crate::event::{Event, EventType};
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
            // TODO: zero timestamps handling (chopper?)
            if event.evtype == EventType::Neutron {
                self.id_buffer.push(event.channel.0);
                self.offset_buffer.push(event.rel_time.into());
            }
        }
        if self.id_buffer.len() >= Self::BUFFER_SIZE {
            self.write_chunk()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ModuleId;
    use crate::event::{test_utils, EventTime};
    use crate::params::{HasParams, ParamMap};

    fn make_common() -> OutputCommon {
        let (_send, recv) = crate::channel::unbounded();
        OutputCommon::new(ModuleId::new("hdf5".into()), recv, None)
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("umami_test_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn dir_config(dir: &std::path::Path) -> toml::Table {
        let mut cfg = toml::Table::new();
        cfg.insert("dir".into(), toml::Value::String(dir.to_string_lossy().into_owned()));
        cfg
    }

    fn neutron_with_rel_time(channel: u32, rel_time_ns: i64) -> Event {
        let mut ev = test_utils::neutron(0, channel);
        ev.rel_time = EventTime(rel_time_ns);
        ev
    }

    #[test]
    fn test_hdf5_output_requires_dir_config() {
        assert!(HDF5EventsOutput::from_config(&make_common(), toml::Table::new()).is_err());
    }

    #[test]
    fn test_hdf5_output_writes_only_neutron_events_in_order() {
        let dir = temp_dir("hdf5_output");
        let mut output = HDF5EventsOutput::from_config(&make_common(), dir_config(&dir)).unwrap();

        output.handle_start_of_run("run1").unwrap();
        output.handle_events(&[
            neutron_with_rel_time(5, 1_000),
            test_utils::edge(0, 1, true), // not a neutron, should be ignored
            neutron_with_rel_time(9, 2_000),
        ]).unwrap();
        output.handle_end_of_run().unwrap();

        let file = hdf5::File::open(dir.join("run1.h5")).unwrap();
        let ids = file.dataset("event_id").unwrap().read_raw::<u32>().unwrap();
        let offsets = file.dataset("event_time_offset").unwrap().read_raw::<f64>().unwrap();
        assert_eq!(ids, vec![5, 9]);
        assert!((offsets[0] - 1e-6).abs() < 1e-12);
        assert!((offsets[1] - 2e-6).abs() < 1e-12);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_hdf5_output_auto_flushes_at_buffer_size() {
        let dir = temp_dir("hdf5_output_flush");
        let mut output = HDF5EventsOutput::from_config(&make_common(), dir_config(&dir)).unwrap();
        output.handle_start_of_run("run1").unwrap();

        let events: Vec<_> = (0..HDF5EventsOutput::BUFFER_SIZE as u32)
            .map(|i| neutron_with_rel_time(i, 0))
            .collect();
        output.handle_events(&events).unwrap();

        // still mid-run, but the buffer should already have auto-flushed to disk
        let size = output.file.as_ref().unwrap().dataset("event_id").unwrap().size();
        assert_eq!(size, HDF5EventsOutput::BUFFER_SIZE);

        output.handle_end_of_run().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_hdf5_output_filename_param_overrides_run_id() {
        let dir = temp_dir("hdf5_output_name");
        let mut output = HDF5EventsOutput::from_config(&make_common(), dir_config(&dir)).unwrap();

        let mut params = ParamMap::new();
        params.insert("filename".into(), serde_json::json!("custom.h5"));
        output.update_params(ModuleId::new("hdf5".into()), params).unwrap();

        output.handle_start_of_run("run1").unwrap();
        output.handle_events(&[neutron_with_rel_time(1, 0)]).unwrap();
        output.handle_end_of_run().unwrap();

        assert!(dir.join("custom.h5").exists());
        assert!(!dir.join("run1.h5").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
