// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod kws;
mod mesy;
mod tof;

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::Event;

/// A "recipe" is an instruction of how to "cook" raw events into events with logical
/// meaning, assigning them to X/Y coordinates and signal meanings.
pub trait Recipe : Send {
    fn from_config(cfg: toml::Table, all: &BTreeMap<String, RecipeConfig>) -> UResult<Self>
        where Self: Sized;
    fn update_config(&mut self, cfg: toml::Table) -> UResult<()>;
    fn process(&mut self, events: Vec<Event>) -> Vec<Event>;
}


pub struct NoRecipe;

impl Recipe for NoRecipe {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(NoRecipe)
    }

    fn update_config(&mut self, _: toml::Table) -> UResult<()> {
        Ok(())
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Event> {
        events
    }
}


pub fn from_config(map: &BTreeMap<String, RecipeConfig>, name: &str) -> UResult<Box<dyn Recipe>> {
    let this = map.get(name).cloned()
                            .ok_or_else(|| anyhow::anyhow!("Recipe {name} not found"))?;
    match this.r#type.as_str() {
        // TODO: should be a macro
        "none" => Ok(Box::new(NoRecipe)),
        "tof_std" => Ok(Box::new(tof::TofStd::from_config(this.config, map)
                                 .with_context(|| format!("Creating recipe {name}"))?)),
        "mesy_test" => Ok(Box::new(mesy::MesyTest::from_config(this.config, map)
                                 .with_context(|| format!("Creating recipe {name}"))?)),
        "kws_ge" => Ok(Box::new(kws::KWSGERecipe::from_config(this.config, map)
                                 .with_context(|| format!("Creating recipe {name}"))?)),
        _ => Err(anyhow!("Unknown recipe type: {}", this.r#type).into()),
    }
}
