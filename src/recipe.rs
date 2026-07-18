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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_utils;
    use crate::event::EventData;

    #[test]
    fn test_no_recipe_passthrough() {
        let mut recipe = NoRecipe::from_config(toml::Table::new(), &BTreeMap::new()).unwrap();
        let events = vec![
            test_utils::neutron(100, 1),
            test_utils::edge(200, 2, true),
        ];
        let out = recipe.process(events);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].data, EventData::Neutron);
        assert_eq!(out[1].data, EventData::Edge { up: true });
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
    fn test_from_config_histo_std() {
        let map = recipe_map("test", "histo_std");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_from_config_histo_tof() {
        let map = recipe_map("test", "histo_tof");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::tzero(0), test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_from_config_mesy_mpsd() {
        let map = recipe_map("test", "mesy_mpsd");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_from_config_mesy_mdll() {
        let map = recipe_map("test", "mesy_mdll");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_from_config_kws_gedet() {
        let map = recipe_map("test", "kws_gedet");
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_from_config_canon() {
        let mut table = toml::Table::new();
        table.insert("reso".into(), toml::Value::Integer(256));
        let map = BTreeMap::from([("test".into(), RecipeConfig {
            r#type: "canon".into(),
            config: table,
        })]);
        let mut r = from_config(&map, "test").unwrap();
        let out = r.process(vec![test_utils::neutron(100, 1)]);
        assert_eq!(out.len(), 1);
    }
}
