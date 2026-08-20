// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use anyhow::{anyhow, Context};
use zerocopy::IntoBytes;
#[cfg(test)]
use zerocopy::TryFromBytes;
use crate::error::UResult;
use crate::event::Event;
use crate::format::Format;
use crate::params::HasParams;
use super::{EventBatch, Output, OutputCommon};

/// `handle_events` flushes the buffered batch to the writer once it grows
/// past this size.
const EVENT_BATCH_SIZE: usize = 8192;

#[derive(HasParams)]
#[params(kind = "output", type = "file")]
pub struct FileOutput<F: Format> {
    // Configuration
    #[param(help="Directory to write raw event files to")]
    dir: PathBuf,
    #[param(help="Filename to write raw events, within 'dir', null to use run no",
            datatype="null or string")]
    filename: Option<String>,
    // Runtime
    writer: Option<BufWriter<File>>,
    buffer: EventBatch<F>,
}

impl<F: Format> FileOutput<F> {
    const BUFFER_SIZE: usize = 1 << 15;

    fn flush(&mut self, batch: Vec<F>) -> UResult<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.write_all(batch.as_bytes()).context("Writing binary events")?;
        }
        Ok(())
    }
}

impl<F: Format> Output for FileOutput<F> {
    fn from_config(_: &OutputCommon, config: toml::Table) -> UResult<Self> where Self: Sized {
        let dir = config.get("dir")
            .ok_or_else(|| anyhow!("Missing 'dir' in file output config"))?
            .as_str()
            .ok_or_else(|| anyhow!("'dir' in file output config must be a string"))?;
        Ok(FileOutput { writer: None, filename: None, dir: PathBuf::from(dir),
                        buffer: EventBatch::new(EVENT_BATCH_SIZE) })
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        let filename = self.filename.as_deref().unwrap_or(run);
        let path = self.dir.join(filename);
        let file = File::create(&path)
            .with_context(|| format!("Creating output file {}", path.display()))?;
        let buffered = BufWriter::with_capacity(Self::BUFFER_SIZE, file);
        self.writer = Some(buffered);
        self.buffer.clear();
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        if let Some(batch) = self.buffer.take_remainder() {
            self.flush(batch)?;
        }
        self.writer = None;
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        self.buffer.push(events.iter().copied());
        if let Some(batch) = self.buffer.take_if_full() {
            self.flush(batch)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ModuleId;
    use crate::event::test_utils;
    use crate::format::Full;
    use crate::params::{HasParams, ParamMap};

    fn make_common() -> OutputCommon {
        let (_send, recv) = crate::channel::unbounded();
        OutputCommon::new(ModuleId::new("file".into()), "umami".into(), recv, None,
                          std::sync::Arc::new(crate::expr::AliasTable::new()))
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

    #[test]
    fn test_file_output_requires_dir_config() {
        assert!(FileOutput::<Full>::from_config(&make_common(), toml::Table::new()).is_err());
    }

    #[test]
    fn test_file_output_writes_and_roundtrips_event() {
        let dir = temp_dir("file_output");
        let mut output = FileOutput::<Full>::from_config(&make_common(), dir_config(&dir)).unwrap();

        // events before a run has started are silently dropped, not an error
        output.handle_events(&[test_utils::neutron(100, 5)]).unwrap();
        assert!(!dir.join("run1").exists());

        output.handle_start_of_run("run1").unwrap();
        let event = test_utils::neutron(200, 7);
        output.handle_events(&[event]).unwrap();
        output.handle_end_of_run().unwrap();

        let bytes = std::fs::read(dir.join("run1")).unwrap();
        let restored = Full::try_read_from_bytes(&bytes).unwrap();
        assert_eq!(restored, Full::from_event(event));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_file_output_filename_param_overrides_run_id() {
        let dir = temp_dir("file_output_name");
        let mut output = FileOutput::<Full>::from_config(&make_common(), dir_config(&dir)).unwrap();

        let mut params = ParamMap::new();
        params.insert("filename".into(), serde_json::json!("custom.dat"));
        output.update_params(ModuleId::new("file".into()), params).unwrap();

        output.handle_start_of_run("run1").unwrap();
        output.handle_events(&[test_utils::neutron(100, 5)]).unwrap();
        output.handle_end_of_run().unwrap();

        assert!(dir.join("custom.dat").exists());
        assert!(!dir.join("run1").exists());

        std::fs::remove_dir_all(&dir).ok();
    }
}
