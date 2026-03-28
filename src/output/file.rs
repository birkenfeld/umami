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
use super::Output;

#[derive(HasParams)]
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
    fn from_config(config: toml::Table) -> UResult<Self> where Self: Sized {
        let dir = config.get("dir")
            .ok_or_else(|| anyhow!("Missing 'dir' in file output config"))?
            .as_str()
            .ok_or_else(|| anyhow!("'dir' in file output config must be a string"))?;
        Ok(FileOutput { writer: None, filename: None, dir: PathBuf::from(dir) })
    }

    // TODO: config api to set the filename

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
