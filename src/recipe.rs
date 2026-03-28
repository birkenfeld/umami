// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod kws;
mod mesy;
mod canon;
mod histo;

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::Event;
use crate::params::HasParams;

/// A "recipe" is an instruction of how to "cook" raw events into events with logical
/// meaning, assigning them to X/Y coordinates and signal meanings.
pub trait Recipe : Send + HasParams {
    fn from_config(cfg: toml::Table, all: &BTreeMap<String, RecipeConfig>) -> UResult<Self>
        where Self: Sized;
    fn process(&mut self, events: Vec<Event>) -> Vec<Event>;
}


#[derive(HasParams)]
pub struct NoRecipe {}

impl Recipe for NoRecipe {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(NoRecipe {})
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Event> {
        events
    }
}


pub fn from_config(map: &BTreeMap<String, RecipeConfig>, name: &str)
                   -> UResult<Box<dyn Recipe>> {
    let this = map.get(name).cloned()
                            .ok_or_else(|| anyhow::anyhow!("Recipe {name} not found"))?;

    macro_rules! recipes {
        ($($name:literal => $typ:ty,)*) => {
            match this.r#type.as_str() {
                "none" => Ok(Box::new(NoRecipe {})),
                $(
                    $name => Ok(Box::new(
                        <$typ>::from_config(this.config, map)
                            .with_context(|| format!("Creating recipe {name}"))?
                    )),
                )*
                _ => Err(anyhow!("Unknown recipe type: {}", this.r#type).into()),
            }
        }
    }

    recipes! {
        "histo_std" => histo::Std,
        "histo_tof" => histo::Tof,
        "mesy_mdll" => mesy::Mdll,
        "mesy_mpsd" => mesy::Mpsd,
        "canon" => canon::Psd,
        "kws_gedet" => kws::KWSGERecipe,
    }
}
