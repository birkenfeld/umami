// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use crate::channel::{Receiver, Sender};
use crate::command::ModuleId;
use crate::error::UResult;
use crate::event::Event;
use crate::params::HasParams;

static TEST_DONE_TX: std::sync::Mutex<std::collections::BTreeMap<String, Sender<()>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

pub fn init_test_output(run_id: &str) -> Receiver<()> {
    let (tx, rx) = crate::channel::bounded(1);
    TEST_DONE_TX.lock().unwrap().insert(run_id.to_string(), tx);
    rx
}

pub struct TestOutput {
    current_run: String,
}

impl TestOutput {
    pub fn new() -> Self {
        TestOutput { current_run: String::new() }
    }
}

impl HasParams for TestOutput {
    fn get_params(&self) -> UResult<crate::params::ParamMap> {
        Ok(crate::params::ParamMap::new())
    }
    fn update_params(&mut self, _: ModuleId, _: crate::params::ParamMap) -> UResult<()> {
        Ok(())
    }
}

impl super::Output for TestOutput {
    fn from_config(_: &super::OutputCommon, _: toml::Table) -> UResult<Self> {
        unreachable!()
    }
    fn handle_events(&mut self, _: &[Event]) -> UResult<()> { Ok(()) }
    fn handle_start_of_run(&mut self, run: &str) -> UResult<()> {
        self.current_run = run.to_string();
        Ok(())
    }
    fn handle_end_of_run(&mut self) -> UResult<()> {
        if let Some(tx) = TEST_DONE_TX.lock().unwrap().get(&self.current_run) {
            tx.send(()).ok();
        }
        Ok(())
    }
}
