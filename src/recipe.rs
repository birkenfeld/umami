// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod kws;
mod tof;

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use serde::Deserialize;
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::Event;

/// A "recipe" is an instruction of how to "cook" raw events into events with logical
/// meaning, assigning them to X/Y coordinates and signal meanings.
pub trait Recipe : Send {
    type Config: Deserialize<'static> where Self: Sized;
    fn from_config(cfg: Self::Config, all: &BTreeMap<String, RecipeConfig>) -> Self where Self: Sized;

    fn process(&mut self, events: Vec<Event>) -> Vec<Event>;
}


pub struct NoRecipe;

impl Recipe for NoRecipe {
    type Config = ();

    fn from_config(_: Self::Config, _: &BTreeMap<String, RecipeConfig>) -> Self {
        NoRecipe
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Event> {
        events
    }
}


pub fn from_config(map: &BTreeMap<String, RecipeConfig>, name: &str) -> UResult<Box<dyn Recipe>> {
    let this = map.get(name).cloned()
                            .ok_or_else(|| anyhow::anyhow!("Recipe {name} not found"))?;
    match this.r#type.as_str() {
        "none" => Ok(Box::new(NoRecipe)),
        "tof_std" => Ok(Box::new(tof::TofStd::from_config((), map))),
        "kws_ge" => Ok(Box::new(kws::KWSGERecipe::from_config(
            this.config.try_into().context("parsing config for kws_ge recipe")?,
            map
        ))),
        _ => Err(anyhow!("Unknown recipe type: {}", this.r#type).into()),
    }
}
