// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

mod kws;
mod mesy;
mod canon;
mod histo;
mod jumiom;

use std::collections::BTreeMap;
use anyhow::{anyhow, Context};
use crate::config::RecipeConfig;
use crate::error::UResult;
use crate::event::Event;
use crate::expr::ExprAlias;
use crate::params::HasParams;

/// A "recipe" is an instruction of how to "cook" raw events into events with logical
/// meaning, assigning them to X/Y coordinates and signal meanings.
pub trait Recipe : Send + HasParams {
    fn from_config(cfg: toml::Table, all: &BTreeMap<String, RecipeConfig>) -> UResult<Self>
        where Self: Sized;
    fn process(&mut self, events: Vec<Event>) -> Vec<Event>;

    /// Called at the start of every run, for every configured recipe, not
    /// just the active one.
    fn start_of_run(&mut self) {}

    /// Named aux-histo expression aliases this recipe contributes.
    fn expr_aliases(&self) -> Vec<ExprAlias> {
        Vec::new()
    }
}


/// A null recipe - does nothing to the events.
#[derive(HasParams)]
#[params(kind = "recipe", type = "none")]
pub struct NoRecipe {}

impl Recipe for NoRecipe {
    fn from_config(_: toml::Table, _: &BTreeMap<String, RecipeConfig>) -> UResult<Self> {
        Ok(NoRecipe {})
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Event> {
        events
    }
}


/// Create a recipe from the given config map and recipe name.
pub fn from_config(map: &BTreeMap<String, RecipeConfig>, name: &str)
                   -> UResult<Box<dyn Recipe>> {
    let this = map.get(name).cloned()
                            .ok_or_else(|| anyhow::anyhow!("Recipe {name} not found"))?;

    macro_rules! recipes {
        ($($typ:ty),* $(,)?) => {
            match this.r#type.as_str() {
                NoRecipe::TYPE_NAME => Ok(Box::new(NoRecipe {})),
                $(
                    <$typ>::TYPE_NAME => Ok(Box::new(
                        <$typ>::from_config(this.config, map)
                            .with_context(|| format!("Creating recipe {name}"))?
                    )),
                )*
                _ => Err(anyhow!("Unknown recipe type: {}", this.r#type).into()),
            }
        }
    }

    recipes! {
        histo::Std,
        histo::Tof,
        mesy::Mdll,
        mesy::Mpsd,
        canon::Psd,
        kws::KWSGERecipe,
        jumiom::Position,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;
    use crate::event::EventType;

    #[test]
    fn test_no_recipe_passthrough() {
        let mut recipe = NoRecipe::from_config(toml::Table::new(), &BTreeMap::new()).unwrap();
        let events = vec![
            test_utils::neutron(100, 1),
            test_utils::edge(200, 2, true),
        ];
        let out = recipe.process(events);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].evtype, EventType::Neutron);
        assert_eq!(out[1].evtype, EventType::Edge { up: true });
    }

    fn recipe_map(name: &str, r#type: &str) -> BTreeMap<String, RecipeConfig> {
        let mut map = BTreeMap::new();
        map.insert(name.to_string(), RecipeConfig {
            r#type: r#type.to_string(),
            config: toml::Table::new(),
        });
        map
    }

    #[test]
    fn test_from_config_none() {
        let map = recipe_map("test", "none");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_from_config_not_found() {
        let map = BTreeMap::new();
        match from_config(&map, "missing") {
            Ok(_) => panic!("expected error"),
            Err(e) => assert!(e.to_string().contains("not found")),
        }
    }

    #[test]
    fn test_from_config_unknown_type() {
        let map = recipe_map("test", "bogus");
        match from_config(&map, "test") {
            Ok(_) => panic!("expected error"),
            Err(e) => assert!(e.to_string().contains("Unknown recipe type")),
        }
    }

    #[test]
    fn test_from_config_dispatches_to_recipe_type() {
        // (recipe type, extra config it needs beyond defaults)
        let cases: &[(&str, &[(&str, toml::Value)])] = &[
            ("histo_std", &[]),
            ("histo_tof", &[]),
            ("mesy_mpsd", &[]),
            ("mesy_mdll", &[]),
            ("kws_gedet", &[]),
            ("canon", &[("reso", toml::Value::Integer(256))]),
            ("jumiom", &[]),
        ];
        for (r#type, extra) in cases {
            let mut config = toml::Table::new();
            for (key, value) in *extra {
                config.insert((*key).to_string(), value.clone());
            }
            let map = BTreeMap::from([("test".to_string(),
                                       RecipeConfig { r#type: (*r#type).to_string(), config })]);
            let mut recipe = from_config(&map, "test")
                .unwrap_or_else(|e| panic!("building recipe {type}: {e:#}", type = r#type));
            // one Tzero and one Neutron event covers both kinds a recipe might special-case
            let out = recipe.process(vec![test_utils::tzero(0), test_utils::neutron(100, 1)]);
            assert_eq!(out.len(), 2, "recipe {type} should pass through both events");
        }
    }
}
