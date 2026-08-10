// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

use std::collections::{BTreeMap, HashMap};
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

/// For the "string to IP/File variant", anything that contains exactly one
/// colon but no slashes is a valid IP/port combination.
fn deserialize_ip<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if !s.contains('/') && s.chars().filter(|&c| c == ':').count() == 1 {
        Ok(s)
    } else {
        Err(serde::de::Error::custom("Expected an IP address (string containing one ':')"))
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

/// Patches `(module, param) -> value` entries into the config file at
/// `source_path`, preserving existing formatting/comments, and writes the
/// result to `target_path` (which may be the same path). Used by
/// `SaveConfig` to persist a `set-params`-tweaked runtime state.
pub fn patch_config_file(
    source_path: &Path,
    target_path: &Path,
    updates: &HashMap<(&str, &str), &serde_json::Value>,
) -> UResult<()> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("Failed to read config file {source_path:?}"))?;
    let mut doc: toml_edit::DocumentMut = content.parse()
        .with_context(|| format!("Failed to parse config file {source_path:?}"))?;

    for (&(module, param), &value) in updates {
        patch_param(&mut doc, module, param, value);
    }

    std::fs::write(target_path, doc.to_string())
        .with_context(|| format!("Failed to write config file {target_path:?}"))?;
    Ok(())
}

/// Sets `<module>.<param> = value` in `doc`, searching every top-level
/// section a module name can live in. No-ops (silently) if `module` isn't
/// found anywhere -- e.g. the auto-created "null" output that has no entry
/// in the original file.
fn patch_param(doc: &mut toml_edit::DocumentMut, module: &str, param: &str, value: &serde_json::Value) {
    for section in ["inputs", "input_recipes", "process_modes", "outputs"] {
        if let Some(table) = doc.get_mut(section)
            .and_then(|s| s.as_table_like_mut())
            .and_then(|t| t.get_mut(module))
            .and_then(|m| m.as_table_like_mut())
        {
            table.insert(param, toml_edit::Item::Value(json_to_toml(value)));
            return;
        }
    }
}

/// Converts a `get-params` JSON value into the equivalent `toml_edit` value.
/// Compound (map/array) values become inline tables/arrays -- this doesn't
/// try to preserve any existing nested-table formatting for them, only the
/// surrounding file's.
fn json_to_toml(value: &serde_json::Value) -> toml_edit::Value {
    match value {
        serde_json::Value::Null => toml_edit::Value::from(""),
        serde_json::Value::Bool(b) => toml_edit::Value::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml_edit::Value::from(i)
            } else {
                toml_edit::Value::from(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => toml_edit::Value::from(s.as_str()),
        serde_json::Value::Array(arr) => {
            toml_edit::Value::from(arr.iter().map(json_to_toml).collect::<toml_edit::Array>())
        }
        serde_json::Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (k, v) in map {
                table.insert(k, json_to_toml(v));
            }
            toml_edit::Value::from(table)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A param passed in `updates` is patched in; one that isn't (the
    /// caller's job to decide, e.g. because it's unchanged this session)
    /// keeps its original formatting untouched, as does unrelated content
    /// (comments, other sections).
    #[test]
    fn test_patch_config_file_writes_given_params_and_leaves_rest_alone() {
        let dir = std::env::temp_dir();
        let source = dir.join(format!("umami_patch_test_source_{}.conf", std::process::id()));
        let target = dir.join(format!("umami_patch_test_target_{}.conf", std::process::id()));
        std::fs::write(&source, r#"
# a comment that should survive
[inputs.mcpd0]
type = "mesy"
cells    =    { 1 = { source = "aux2", compare = 5 } }
modules = {}
"#).unwrap();

        let changed_modules = serde_json::json!({"0": {"type": "mpsd", "threshold": 7, "gain": 3}});
        let updates = HashMap::from([
            (("mcpd0", "modules"), &changed_modules),
        ]);
        patch_config_file(&source, &target, &updates).unwrap();

        let saved = std::fs::read_to_string(&target).unwrap();
        std::fs::remove_file(&source).ok();
        std::fs::remove_file(&target).ok();
        assert!(saved.contains("a comment that should survive"));
        assert!(saved.contains("cells    =    { 1 = { source = \"aux2\", compare = 5 } }"),
                "saved:\n{saved}");
        let doc: toml_edit::DocumentMut = saved.parse().unwrap();
        assert_eq!(doc["inputs"]["mcpd0"]["modules"]["0"]["threshold"].as_integer(), Some(7));
    }

    #[test]
    fn test_source_config_deserialize() {
        let tbl: toml::Table = toml::from_str(r#"
            val1 = "localhost:50001"
            val2 = "file"
            val3 = "/data:archive/file.dump"
            val4 = "fe80::1:50001"
        "#).unwrap();
        let cfg: SourceConfig = tbl["val1"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::IP(s) if s == "localhost:50001"));
        let cfg: SourceConfig = tbl["val2"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::File(s) if s == "file"));
        // a file path containing a colon (a legal Unix filename character)
        // must not be misdetected as a network address
        let cfg: SourceConfig = tbl["val3"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::File(s) if s == "/data:archive/file.dump"));
        // multiple colons (e.g. an IPv6-ish address) isn't a supported
        // "host:port" form either, and falls back to File
        let cfg: SourceConfig = tbl["val4"].clone().try_into().unwrap();
        assert!(matches!(cfg, SourceConfig::File(s) if s == "fe80::1:50001"));
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
