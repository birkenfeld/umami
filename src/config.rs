// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use anyhow::Context;
use serde::Deserialize;
use crate::error::UResult;
use crate::input::canon::CanonConfig;
use crate::input::ge::GEConfig;
use crate::input::mesy::MesyConfig;
#[cfg(feature = "jumiom")]
use crate::input::jumiom::JumiomConfig;
#[cfg(test)]
use crate::input::test::TestInputConfig;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SourceConfig {
    IP(#[serde(deserialize_with = "deserialize_ip")] String),
    File(String),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SpecificInputConfig {
    GE(GEConfig),
    Canon(CanonConfig),
    Mesy(MesyConfig),
    #[cfg(feature = "jumiom")]
    Jumiom(JumiomConfig),
    #[cfg(test)]
    Test(TestInputConfig),
}

#[derive(Debug, Deserialize)]
pub struct InputConfig {
    pub recipe: String,
    #[serde(flatten)]
    pub specific: SpecificInputConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RecipeConfig {
    pub r#type: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OutputConfig {
    pub r#type: String,
    #[serde(flatten)]
    pub config: toml::Table,
}

#[derive(Debug, Deserialize)]
pub struct ProcessModesConfig {
    pub default: String,
    #[serde(flatten)]
    pub recipes: BTreeMap<String, RecipeConfig>,
}

#[derive(Debug, Deserialize)]
pub struct HistoConfig {
    pub nx: usize,
    pub ny: usize,
    pub max_nt: usize,
    #[serde(default)]
    pub max_ni: usize,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Input modules, by name.
    pub inputs: BTreeMap<String, InputConfig>,
    /// Recipes for processing input events, by name.  Each input module
    /// specifies exactly one recipe to use for its events - usually it
    /// is the same for all inputs of a certain type.
    pub input_recipes: BTreeMap<String, RecipeConfig>,
    /// Processing modes, by name.  Each processing mode is a recipe
    /// that is applied after events are read and sorted.
    pub process_modes: ProcessModesConfig,
    /// Histogram configuration (size, max events per bin, etc.).
    pub histogram: HistoConfig,
    /// Output modules, by name.
    pub outputs: Option<BTreeMap<String, OutputConfig>>,
    /// IPC name - shared memory segment and Unix socket name.
    #[serde(default = "default_ipc_name")]
    pub ipc_name: String,
    /// Optional name of the detector config, cosmetic only.
    pub name: Option<String>,
    /// Optional path to a directory where raw event dumps are written.
    pub raw_dir: Option<PathBuf>,
    /// Debug mode: print extra info to stdout.
    #[serde(default)]
    pub debug: bool,
    /// User-defined expression aliases for aux histograms.
    #[serde(default)]
    pub expr_aliases: BTreeMap<String, ExprAliasConfig>,
    /// Path to the config file, filled in by `load_config()`.
    #[serde(skip)]
    pub filename: PathBuf,
}

/// Deserialized from either a plain string (the expression, no help text) or a
/// table with an optional `help`.
#[derive(Debug, Clone)]
pub struct ExprAliasConfig {
    pub expr: String,
    pub help: String,
}

impl<'de> Deserialize<'de> for ExprAliasConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de>
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum Repr {
            Short(String),
            Full {
                expr: String,
                #[serde(default)]
                help: String,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Short(expr) => ExprAliasConfig { expr, help: String::new() },
            Repr::Full { expr, help } => ExprAliasConfig { expr, help },
        })
    }
}

fn deserialize_ip<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.contains(':') {
        Ok(s)
    } else {
        Err(serde::de::Error::custom("Expected an IP address (string containing ':')"))
    }
}

fn default_ipc_name() -> String {
    "umami".into()
}

pub fn load_config(path: &Path) -> UResult<Config> {
    let mut config: Config = toml::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file {:?}", path.display()))?
    ).with_context(|| format!("Failed to parse config file {:?}", path.display()))?;
    config.filename = path.canonicalize().context("Failed to get canonical config path")?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_config_deserialize() {
        let tbl: toml::Table = toml::from_str(r#"
            val1 = "localhost:50001"
            val2 = "/path/to/file"
        "#).unwrap();
        let cfg: SourceConfig = tbl["val1"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::IP(s) if s == "localhost:50001"));
        let cfg: SourceConfig = tbl["val2"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::File(s) if s == "/path/to/file"));
    }

    #[test]
    fn test_full_config_minimal() {
        let cfg: Config = toml::from_str(r#"
            [inputs.main]
            id = 0
            recipe = "none"
            type = "ge"
            source = "localhost:50001"

            [input_recipes]
            none = { type = "none", blah = 5 }

            [process_modes]
            default = "std"
            std = { type = "histo_std", bin_x = 2 }

            [histogram]
            nx = 100
            ny = 100
            max_nt = 10
        "#).unwrap();
        assert_eq!(cfg.inputs.len(), 1);
        assert!(cfg.inputs.contains_key("main"));
        assert_eq!(cfg.ipc_name, "umami");
        assert!(!cfg.debug);
        assert!(cfg.outputs.is_none());
        assert_eq!(cfg.process_modes.default, "std");
        assert!(cfg.process_modes.recipes.contains_key("std"));
        let proc = cfg.process_modes.recipes.get("std").unwrap();
        assert_eq!(proc.r#type, "histo_std");
        assert_eq!(proc.config.get("bin_x").and_then(|v| v.as_integer()), Some(2));
        let rec = cfg.input_recipes.get("none").unwrap();
        assert_eq!(rec.r#type, "none");
        assert_eq!(rec.config.get("blah").and_then(|v| v.as_integer()), Some(5));
    }
}
