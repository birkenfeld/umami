// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use anyhow::{anyhow, Context};
use rkyv::{api::high::to_bytes_in, ser::writer::IoWriter};
use crate::error::UResult;
use crate::event::Event;
use crate::params::HasParams;
use super::{Output, OutputCommon};

#[derive(HasParams)]
#[params(kind = "output", type = "file")]
pub struct FileOutput {
    // Configuration
    #[param(help="Directory to write raw event files to")]
    dir: PathBuf,
    #[param(help="Filename to write raw events, within 'dir', null to use run no",
            datatype="null or string")]
    filename: Option<String>,
    // Runtime
    writer: Option<IoWriter<BufWriter<File>>>,
}

impl FileOutput {
    const BUFFER_SIZE: usize = 1 << 15;
}

impl Output for FileOutput {
    fn from_config(_: &OutputCommon, config: toml::Table) -> UResult<Self> where Self: Sized {
        let dir = config.get("dir")
            .ok_or_else(|| anyhow!("Missing 'dir' in file output config"))?
            .as_str()
            .ok_or_else(|| anyhow!("'dir' in file output config must be a string"))?;
        Ok(FileOutput { writer: None, filename: None, dir: PathBuf::from(dir) })
    }

    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        let filename = self.filename.as_deref().unwrap_or(run);
        let path = self.dir.join(filename);
        let file = File::create(&path)
            .with_context(|| format!("Creating output file {}", path.display()))?;
        let buffered = BufWriter::with_capacity(Self::BUFFER_SIZE, file);
        self.writer = Some(IoWriter::new(buffered));
        Ok(())
    }

    fn handle_end_of_run(&mut self) -> UResult<()> {
        self.writer = None;
        Ok(())
    }

    fn handle_events(&mut self, events: &[Event]) -> UResult<()> {
        if let Some(mut writer) = self.writer.as_mut() {
            for event in events {
                to_bytes_in::<_, rkyv::rancor::Failure>(event, &mut writer)
                    .context("Serializing event for file output")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::ModuleId;
    use crate::event::test_utils;
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
        assert!(FileOutput::from_config(&make_common(), toml::Table::new()).is_err());
    }

    #[test]
    fn test_file_output_writes_and_roundtrips_event() {
        let dir = temp_dir("file_output");
        let mut output = FileOutput::from_config(&make_common(), dir_config(&dir)).unwrap();

        // events before a run has started are silently dropped, not an error
        output.handle_events(&[test_utils::neutron(100, 5)]).unwrap();
        assert!(!dir.join("run1").exists());

        output.handle_start_of_run("run1").unwrap();
        let event = test_utils::neutron(200, 7);
        output.handle_events(&[event]).unwrap();
        output.handle_end_of_run().unwrap();

        let bytes = std::fs::read(dir.join("run1")).unwrap();
        let restored: Event = rkyv::from_bytes::<Event, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(restored, event);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_file_output_filename_param_overrides_run_id() {
        let dir = temp_dir("file_output_name");
        let mut output = FileOutput::from_config(&make_common(), dir_config(&dir)).unwrap();

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
