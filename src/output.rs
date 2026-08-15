// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::sync::Arc;
use anyhow::{anyhow, Context};
use crate::lprintln;
use crate::channel::{Receiver, Sender};
use crate::command::{CommandReply, ModuleId};
use crate::config::OutputConfig;
use crate::error::UResult;
use crate::event::Event;
use crate::expr::AliasTable;
use crate::params::HasParams;
use crate::pipeline::PipeItem;

mod aux_histo;
mod diag;
mod ext_process;
mod file;
#[cfg(feature = "hdf5")]
mod hdf5;
#[cfg(test)]
pub(crate) mod test;

pub struct OutputCommon {
    name: ModuleId,
    ipc_name: String,
    input: Receiver<PipeItem>,
    output: Option<Sender<PipeItem>>,
    expr_aliases: Arc<AliasTable>,
}

impl OutputCommon {
    pub fn new(
        name: ModuleId,
        ipc_name: String,
        input: Receiver<PipeItem>,
        output: Option<Sender<PipeItem>>,
        expr_aliases: Arc<AliasTable>,
    ) -> Self {
        Self { name, ipc_name, input, output, expr_aliases }
    }
}


pub trait Output: Send + HasParams {
    fn from_config(common: &OutputCommon, config: toml::Table) -> UResult<Self>
    where Self: Sized;

    fn handle_events(&mut self, events: &[Event]) -> UResult<()>;
    fn handle_start_of_run(&mut self, run: &str) -> UResult<()>;
    fn handle_end_of_run(&mut self) -> UResult<()>;

    /// Called on `Command::Clear`. Default no-op; most outputs don't hold
    /// clearable state, but e.g. `aux_histo` zeroes its histograms.
    fn handle_clear(&mut self) -> UResult<()> {
        Ok(())
    }

    fn main_loop(mut self, common: OutputCommon)
    where Self: Sized
    {
        let mut current_run = String::new();
        let name = common.name;
        while let Ok(mut item) = common.input.recv() {
            match &mut item {
                PipeItem::Events(events) => {
                    if let Err(e) = self.handle_events(events) {
                        lprintln!(ERROR, [name] "Error handling events: {e:#}");
                    }
                }
                PipeItem::StartOfRun(run) => {
                    current_run = run.clone();
                    if let Err(e) = self.handle_start_of_run(run) {
                        lprintln!(ERROR, [name] "Error handling start of run: {e:#}");
                    }
                }
                PipeItem::EndOfRun => {
                    if let Err(e) = self.handle_end_of_run() {
                        lprintln!(ERROR, [name] "Error handling end of run: {e:#}");
                    }
                    lprintln!(INFO, [name] "Finished with run {:?}", current_run);
                }
                PipeItem::Clear => {
                    if let Err(e) = self.handle_clear() {
                        lprintln!(ERROR, [name] "Error handling clear: {e:#}");
                    }
                }
                PipeItem::GetParams(full, send) => {
                    match self.get_params(*full) {
                        Ok(params) => send.send((common.name, params))
                                          .expect("param reply receiver died"),
                        Err(e) => {
                            lprintln!(ERROR, [name] "Error getting params: {e:#}");
                        }
                    }
                }
                PipeItem::SetParams(param_map, send) => {
                    if let Some(params) = param_map.remove(&common.name) {
                        if let Err(e) = self.update_params(common.name, params) {
                            lprintln!(ERROR, [name] "Error setting parameters: {e:#}");
                            send.send(CommandReply::new_mod_error(
                                common.name,
                                format!("Failed to set parameters: {e:#}")
                            )).expect("param reply receiver died");
                        } else {
                            send.send(CommandReply::Ok).expect("param reply receiver died");
                        }
                    }
                }
                _ => {}
            }
            if let Some(sender) = &common.output {
                let _ = sender.send(item);
            }
        }
    }

    fn start(self, common: OutputCommon) -> UResult<()>
    where Self: Sized + 'static
    {
        lprintln!(INFO, [common.name] "Initialized output");
        std::thread::Builder::new()
            .name(format!("O: {}", common.name))
            .spawn(move || self.main_loop(common))
            .context("Spawning output thread")?;
        Ok(())
    }
}

/// Absorb all events without action.
#[derive(HasParams)]
#[params(kind = "output", type = "none")]
pub struct NullOutput {}

impl Output for NullOutput {
    fn from_config(_: &OutputCommon, _: toml::Table) -> UResult<Self> {
        Ok(NullOutput {})
    }

    fn handle_start_of_run(&mut self, _run: &str) -> UResult<()> {
        Ok(())
    }
    fn handle_end_of_run(&mut self) -> UResult<()> {
        Ok(())
    }

    fn handle_events(&mut self, _events: &[Event]) -> UResult<()> {
        Ok(())
    }
}

// Broadcast over udp/uds
// pub struct UDPOutput;
// pub struct UDSOutput;

pub fn start(config: OutputConfig, common: OutputCommon) -> UResult<()> {
    match config.r#type.as_str() {
        NullOutput::TYPE_NAME => Ok(NullOutput::from_config(&common, config.config)?.start(common)?),
        diag::DiagOutput::TYPE_NAME =>
            Ok(diag::DiagOutput::from_config(&common, config.config)?.start(common)?),
        #[cfg(feature = "hdf5")]
        hdf5::HDF5EventsOutput::TYPE_NAME =>
            Ok(hdf5::HDF5EventsOutput::from_config(&common, config.config)?.start(common)?),
        #[cfg(not(feature = "hdf5"))]
        "hdf5" => Err(anyhow!(
            "HDF5 output support was not compiled in (rebuild with --features hdf5)").into()),
        file::FileOutput::TYPE_NAME =>
            Ok(file::FileOutput::from_config(&common, config.config)?.start(common)?),
        aux_histo::AuxHistoOutput::TYPE_NAME =>
            Ok(aux_histo::AuxHistoOutput::from_config(&common, config.config)?.start(common)?),
        ext_process::ExtProcessOutput::TYPE_NAME =>
            Ok(ext_process::ExtProcessOutput::from_config(&common, config.config)?.start(common)?),
        #[cfg(test)]
        test::TestOutput::TYPE_NAME => Ok(test::TestOutput::new().start(common)?),
        _ => Err(anyhow!("Unknown output type: {}", config.r#type).into()),
    }
}
