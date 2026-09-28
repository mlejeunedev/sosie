//! Loading and validation of `sosie.yaml`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};

/// Presets available in v0.1 (see `src/presets`). Used to validate rules and
/// suggest a fix for typos.
pub use crate::presets::KNOWN_PRESETS;

/// Parsed and validated contents of `sosie.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Config format version.
    pub version: u32,
    pub source: Source,
    pub mode: Mode,
    #[serde(default)]
    pub defaults: Defaults,
    /// Per-table, per-column transformation rules: `tables.<table>.<column>`.
    #[serde(default)]
    pub tables: BTreeMap<String, BTreeMap<String, Rule>>,
    /// Tables emitted with their structure but no rows.
    #[serde(default)]
    pub skip_tables: Vec<String>,
    /// Same effect as `skip_tables` in v0.1.
    #[serde(default)]
    pub truncate_tables: Vec<String>,
    /// Ambiguous `table.column` entries left by `init`; must be empty for
    /// `check` to pass.
    #[serde(default)]
    pub review: Vec<String>,
    #[serde(default)]
    pub sampling: Option<Sampling>,
}

/// Database the dump comes from. Only `mysql` is supported in v0.1.
/// Connection details (DSN, credentials) never belong here: they come from
/// environment variables.
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// HMAC key generated randomly per run and discarded: output differs on
    /// every run.
    Anonymize,
    /// HMAC key read from `SOSIE_KEY`: output is stable across runs.
    Pseudonymize,
}

/// What to do with a column that has no rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnclassified {
    /// Refuse to proceed (safe default).
    #[default]
    Fail,
    /// Leave the column untouched.
    Keep,
}

/// Global settings applied when a rule does not override them.
#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    /// Locale used to generate fake data. Only `fr_FR` in v0.1.
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub on_unclassified: OnUnclassified,
}

fn default_locale() -> String {
    "fr_FR".to_string()
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            locale: default_locale(),
            on_unclassified: OnUnclassified::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sampling {
    #[serde(default)]
    pub rate: Option<f64>,
}

/// Transformation rule for a column. Written either as a short form (`keep`,
/// `null`, `constant("…")`, a bare preset name) or as a map
/// `{ preset: …, ...params }`.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Keep,
    Null,
    Constant(String),
    Preset {
        name: String,
        params: BTreeMap<String, serde_yaml::Value>,
    },
}

impl<'de> Deserialize<'de> for Rule {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RuleVisitor;

        impl<'de> Visitor<'de> for RuleVisitor {
            type Value = Rule;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(
                    f,
                    "null, a string (\"email\", \"keep\", \"null\", `constant(\"...\")`) or a map {{ preset: ..., ... }}"
                )
            }

            fn visit_unit<E>(self) -> std::result::Result<Rule, E>
            where
                E: de::Error,
            {
                Ok(Rule::Null)
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Rule, E>
            where
                E: de::Error,
            {
                parse_rule_str(v).map_err(de::Error::custom)
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Rule, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut preset = None;
                let mut params = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "preset" {
                        preset = Some(map.next_value::<String>()?);
                    } else {
                        params.insert(key, map.next_value::<serde_yaml::Value>()?);
                    }
                }
                let name = preset.ok_or_else(|| de::Error::missing_field("preset"))?;
                Ok(Rule::Preset { name, params })
            }
        }

        deserializer.deserialize_any(RuleVisitor)
    }
}

/// Parses the short string form of a rule; anything unrecognized is treated
/// as a preset name (validated later in `Config::validate`).
fn parse_rule_str(s: &str) -> std::result::Result<Rule, String> {
    if s == "keep" {
        return Ok(Rule::Keep);
    }
    if s == "null" {
        return Ok(Rule::Null);
    }
    if let Some(inner) = s
        .strip_prefix("constant(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let inner = inner.trim();
        if inner.len() >= 2 && inner.starts_with('"') && inner.ends_with('"') {
            return Ok(Rule::Constant(inner[1..inner.len() - 1].to_string()));
        }
        return Err(format!(
            "`constant(...)` must contain a quoted string, found: {s}"
        ));
    }
    Ok(Rule::Preset {
        name: s.to_string(),
        params: BTreeMap::new(),
    })
}

/// Secret-bearing keys that must never appear in a versioned config file.
///
/// Only the root and `source` are checked: keys under `tables.*.*` are column
/// names from the app schema, where `password` or `key` are legitimate.
const FORBIDDEN_KEYS: &[&str] = &["dsn", "password", "key"];

fn check_no_secrets(raw: &serde_yaml::Value) -> Result<()> {
    let Some(top) = raw.as_mapping() else {
        return Ok(());
    };
    for (k, v) in top {
        let Some(key) = k.as_str() else { continue };
        if FORBIDDEN_KEYS.contains(&key) {
            bail!(
                "forbidden key `{key}` at the root of the config: never put secrets (dsn/password/key) in it, use an environment variable"
            );
        }
        if key == "source"
            && let Some(source) = v.as_mapping()
        {
            for (sk, _) in source {
                if let Some(skey) = sk.as_str()
                    && FORBIDDEN_KEYS.contains(&skey)
                {
                    bail!(
                        "forbidden key `source.{skey}`: never put secrets (dsn/password/key) in it, use an environment variable"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Levenshtein distance, used to suggest a preset for a typo.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let tmp = row[j];
            row[j] = if a[i - 1] == b[j - 1] {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j - 1])
            };
            prev = tmp;
        }
    }
    row[b.len()]
}

/// Closest known preset within an edit distance of 2, if any.
fn suggest_preset(unknown: &str) -> Option<&'static str> {
    KNOWN_PRESETS
        .iter()
        .map(|&p| (p, levenshtein(unknown, p)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(p, _)| p)
}

fn validate_preset_name(table: &str, column: &str, name: &str) -> Result<()> {
    if KNOWN_PRESETS.contains(&name) {
        return Ok(());
    }
    match suggest_preset(name) {
        Some(suggestion) => {
            bail!("{table}.{column}: unknown preset `{name}` (did you mean `{suggestion}`?)")
        }
        None => bail!(
            "{table}.{column}: unknown preset `{name}` (available presets: {})",
            KNOWN_PRESETS.join(", ")
        ),
    }
}

impl Config {
    /// Reads and parses the config file at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Config> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    /// Parses and validates a YAML config.
    pub fn parse(text: &str) -> Result<Config> {
        let raw: serde_yaml::Value = serde_yaml::from_str(text).context("invalid YAML")?;
        // Checked on the raw YAML, before serde silently drops unknown keys.
        check_no_secrets(&raw)?;

        let config: Config = serde_yaml::from_value(raw).context("invalid config")?;
        config.validate()?;
        Ok(config)
    }

    /// Semantic checks beyond deserialization: v0.1 support limits, preset
    /// names, and `SOSIE_KEY` presence in pseudonymize mode.
    fn validate(&self) -> Result<()> {
        if self.source.kind != "mysql" {
            bail!(
                "source.kind `{}` is not supported in v0.1 (only `mysql` is)",
                self.source.kind
            );
        }

        if self.defaults.locale != "fr_FR" {
            bail!(
                "defaults.locale `{}` is not supported in v0.1 (only `fr_FR` is)",
                self.defaults.locale
            );
        }

        for (table, columns) in &self.tables {
            for (column, rule) in columns {
                if let Rule::Preset { name, .. } = rule {
                    validate_preset_name(table, column, name)?;
                }
            }
        }

        if self.mode == Mode::Pseudonymize {
            match std::env::var("SOSIE_KEY") {
                Ok(key) if key.len() >= 16 => {}
                Ok(_) => bail!("SOSIE_KEY must be at least 16 characters long"),
                Err(_) => bail!(
                    "mode: pseudonymize requires the SOSIE_KEY environment variable (>= 16 characters)"
                ),
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_basic_fixture() {
        let text = include_str!("../../fixtures/exemples/configs/01_basic.yaml");
        let config = Config::parse(text).unwrap();
        assert_eq!(config.mode, Mode::Anonymize);
        assert_eq!(config.defaults.locale, "fr_FR");
        assert_eq!(config.skip_tables, vec!["audit_log".to_string()]);
        assert_eq!(
            config.tables["user"]["email"],
            Rule::Preset {
                name: "email".to_string(),
                params: BTreeMap::new(),
            }
        );
        assert_eq!(config.tables["user"]["api_token"], Rule::Null);
        assert_eq!(config.tables["product"]["name"], Rule::Keep);
        match &config.tables["user"]["password"] {
            Rule::Constant(v) => assert!(v.starts_with("$2y$13$")),
            other => panic!("expected Constant, found {other:?}"),
        }
        match &config.tables["user"]["birth_date"] {
            Rule::Preset { name, params } => {
                assert_eq!(name, "date_shift");
                assert_eq!(params["days"], serde_yaml::Value::from(365));
            }
            other => panic!("expected Preset, found {other:?}"),
        }
    }

    #[test]
    fn loads_incomplete_fixture_without_erroring_at_load_time() {
        // Syntactically valid: missing columns are caught by `check`, not at load time.
        let text = include_str!("../../fixtures/exemples/configs/02_incomplete.yaml");
        Config::parse(text).unwrap();
    }

    #[test]
    fn loads_edge_cases_fixture() {
        let text = include_str!("../../fixtures/exemples/configs/02_parser_edge_cases.yaml");
        let config = Config::parse(text).unwrap();
        assert_eq!(config.tables["order"]["key"], Rule::Keep);
    }

    #[test]
    fn pseudonymize_fixture_requires_sosie_key() {
        let text = include_str!("../../fixtures/exemples/configs/03_pseudonymize.yaml");
        // Make sure SOSIE_KEY is unset.
        unsafe {
            std::env::remove_var("SOSIE_KEY");
        }
        let err = Config::parse(text).unwrap_err();
        assert!(err.to_string().contains("SOSIE_KEY"));
    }

    #[test]
    fn rejects_dsn_field() {
        let text = r#"
version: 1
source: { kind: mysql, dsn: "mysql://user:pass@host/db" }
mode: anonymize
"#;
        let err = Config::parse(text).unwrap_err();
        assert!(err.to_string().contains("dsn"));
    }

    #[test]
    fn rejects_top_level_password_field() {
        let text = r#"
version: 1
source: { kind: mysql }
mode: anonymize
password: "hunter2"
"#;
        let err = Config::parse(text).unwrap_err();
        assert!(err.to_string().contains("password"));
    }

    #[test]
    fn allows_password_as_a_column_name() {
        // `tables.*.*` mirrors the app schema: `password`/`key` columns are legitimate.
        let text = r#"
version: 1
source: { kind: mysql }
mode: anonymize
tables:
  user:
    password: keep
  order:
    key: keep
"#;
        Config::parse(text).unwrap();
    }

    #[test]
    fn unknown_preset_suggests_closest_match() {
        let text = r#"
version: 1
source: { kind: mysql }
mode: anonymize
tables:
  user:
    first_name: fisrt_name
"#;
        let err = Config::parse(text).unwrap_err();
        assert!(err.to_string().contains("first_name"));
    }
}
