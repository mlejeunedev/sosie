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
    /// Rules matching several columns at once: `columns."<table glob>.<column glob>"`,
    /// in file order. An exact `tables` rule takes precedence.
    #[serde(default, deserialize_with = "ordered_patterns")]
    pub columns: Vec<(String, Rule)>,
    /// Tables emitted with their structure but no rows.
    #[serde(default)]
    pub skip_tables: Vec<String>,
    /// Same effect as `skip_tables` in v0.1.
    #[serde(default)]
    pub truncate_tables: Vec<String>,
    /// Ambiguous columns left by `init`, each with a cautious default rule
    /// (`"<table>.<column>": rule`, `*` allowed), in file order. The rules apply,
    /// but `check` fails until every entry is moved to `columns` (rule
    /// confirmed) or to `exclude` (column kept as is). The legacy list form
    /// (`- table.column`) is accepted, without rules.
    #[serde(default, deserialize_with = "review_entries")]
    pub review: Vec<(String, Option<Rule>)>,
    /// Columns deliberately left untouched (`<table>.<column>`, `*` allowed):
    /// a quick `keep` for false positives and harmless data.
    #[serde(default)]
    pub exclude: Vec<String>,
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

/// Keeps `columns:` in file order (a `BTreeMap` would sort it), since the first
/// matching pattern wins.
fn ordered_patterns<'de, D>(deserializer: D) -> std::result::Result<Vec<(String, Rule)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mapping = Option::<serde_yaml::Mapping>::deserialize(deserializer)?.unwrap_or_default();
    mapping
        .into_iter()
        .map(|(k, v)| {
            let pattern = k
                .as_str()
                .ok_or_else(|| de::Error::custom("`columns` keys must be strings"))?
                .to_string();
            let rule = Rule::deserialize(v).map_err(de::Error::custom)?;
            Ok((pattern, rule))
        })
        .collect()
}

/// `review:` as an ordered map of default rules, or the legacy list of
/// `table.column` strings (no rule).
fn review_entries<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<(String, Option<Rule>)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde_yaml::Value>::deserialize(deserializer)? {
        None => Ok(Vec::new()),
        Some(serde_yaml::Value::Sequence(items)) => items
            .into_iter()
            .map(|item| match item {
                serde_yaml::Value::String(key) => Ok((key, None)),
                _ => Err(de::Error::custom("`review` list items must be strings")),
            })
            .collect(),
        Some(value @ serde_yaml::Value::Mapping(_)) => Ok(ordered_patterns(value)
            .map_err(de::Error::custom)?
            .into_iter()
            .map(|(k, r)| (k, Some(r)))
            .collect()),
        Some(_) => Err(de::Error::custom(
            "`review` must be a map of `\"<table>.<column>\": rule`",
        )),
    }
}

/// Where a column is decided. A column must be decided in a single place:
/// rules (`tables`/`columns`), `review` or `exclude`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Rules,
    Review,
    Exclude,
}

impl Decision {
    pub fn section(self) -> &'static str {
        match self {
            Decision::Rules => "tables/columns",
            Decision::Review => "review",
            Decision::Exclude => "exclude",
        }
    }
}

static KEEP: Rule = Rule::Keep;

/// `*`-only glob: `*` matches any run of characters, everything else is literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Whether a `columns` pattern (`"<table glob>.<column glob>"`, split on the
/// first `.`) matches `table.column`.
pub fn pattern_matches(pattern: &str, table: &str, column: &str) -> bool {
    match pattern.split_once('.') {
        Some((t, c)) => glob_match(t, table) && glob_match(c, column),
        None => false,
    }
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

    /// Rule applying to `table.column`: the exact `tables` entry if any, else the
    /// first matching `columns` pattern, else the first matching `review` default,
    /// else `keep` if `exclude` lists it. A column decided in several places is an
    /// error reported by [`Config::decisions_for`], not resolved here.
    pub fn rule_for(&self, table: &str, column: &str) -> Option<&Rule> {
        self.tables
            .get(table)
            .and_then(|cols| cols.get(column))
            .or_else(|| {
                self.columns
                    .iter()
                    .find(|(p, _)| pattern_matches(p, table, column))
                    .map(|(_, r)| r)
            })
            .or_else(|| {
                self.review
                    .iter()
                    .find(|(p, _)| pattern_matches(p, table, column))
                    .and_then(|(_, r)| r.as_ref())
            })
            .or_else(|| {
                self.exclude
                    .iter()
                    .any(|p| pattern_matches(p, table, column))
                    .then_some(&KEEP)
            })
    }

    /// Sections deciding `table.column`. More than one is a conflict: an entry
    /// of `review` or `exclude` must not also have a rule elsewhere.
    pub fn decisions_for(&self, table: &str, column: &str) -> Vec<Decision> {
        let exact = self
            .tables
            .get(table)
            .is_some_and(|c| c.contains_key(column));
        let mut out = Vec::new();
        if exact
            || self
                .columns
                .iter()
                .any(|(p, _)| pattern_matches(p, table, column))
        {
            out.push(Decision::Rules);
        }
        if self
            .review
            .iter()
            .any(|(p, _)| pattern_matches(p, table, column))
        {
            out.push(Decision::Review);
        }
        if self
            .exclude
            .iter()
            .any(|p| pattern_matches(p, table, column))
        {
            out.push(Decision::Exclude);
        }
        out
    }

    /// Every pattern of `columns`, `review` and `exclude`, with its section.
    pub fn patterns(&self) -> impl Iterator<Item = (&str, &'static str)> {
        let columns = self.columns.iter().map(|(p, _)| (p.as_str(), "columns"));
        let review = self.review.iter().map(|(p, _)| (p.as_str(), "review"));
        let exclude = self.exclude.iter().map(|p| (p.as_str(), "exclude"));
        columns.chain(review).chain(exclude)
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

        for (pattern, section) in self.patterns() {
            if !pattern.contains('.') {
                bail!(
                    "{section}: `{pattern}` must have the form `<table>.<column>` (`*` allowed, e.g. `*.description`)"
                );
            }
        }
        let rules = self.columns.iter().map(|(p, r)| ("columns", p, Some(r)));
        let defaults = self.review.iter().map(|(p, r)| ("review", p, r.as_ref()));
        for (section, pattern, rule) in rules.chain(defaults) {
            if let Some(Rule::Preset { name, .. }) = rule {
                validate_preset_name(section, pattern, name)?;
            }
        }

        // The same key in two sections is a conflict visible without the dump
        // (overlapping globs are reported by `check`/`transform`).
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        let exact = self
            .tables
            .iter()
            .flat_map(|(t, cols)| cols.keys().map(move |c| (format!("{t}.{c}"), "tables")));
        let keys = self.patterns().map(|(p, s)| (p.to_string(), s));
        for (key, section) in exact.chain(keys) {
            if let Some(first) = seen.insert(key.clone(), section)
                && first != section
            {
                bail!(
                    "`{key}` is in both `{first}` and `{section}`: decide each column in one place only"
                );
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
    fn glob_matching() {
        assert!(pattern_matches("*.description", "product", "description"));
        assert!(!pattern_matches(
            "*.description",
            "product",
            "short_description"
        ));
        assert!(pattern_matches(
            "*.*_description",
            "product",
            "short_description"
        ));
        assert!(pattern_matches("log_*.*", "log_2024", "payload"));
        assert!(!pattern_matches("log_*.*", "user", "payload"));
        assert!(pattern_matches("*.a*b*c", "t", "aXbYc"));
        assert!(!pattern_matches("*.a*b*c", "t", "aXcYb"));
        assert!(!pattern_matches("*.ab*ba", "t", "aba"));
        assert!(!pattern_matches("description", "t", "description"));
    }

    #[test]
    fn exact_rule_wins_over_pattern_and_first_pattern_wins() {
        let text = r#"
version: 1
source: { kind: mysql }
mode: anonymize
tables:
  product:
    description: keep
columns:
  "*.description": null
  "*.*": keep
  "order.notes": email
"#;
        let config = Config::parse(text).unwrap();
        assert_eq!(config.rule_for("product", "description"), Some(&Rule::Keep));
        assert_eq!(config.rule_for("order", "description"), Some(&Rule::Null));
        // File order, not specificity: "*.*" comes first.
        assert_eq!(config.rule_for("order", "notes"), Some(&Rule::Keep));
    }

    #[test]
    fn review_defaults_apply_and_exclude_keeps() {
        let text = r#"
version: 1
source: { kind: mysql }
mode: anonymize
columns:
  "*.phone": phone
exclude:
  - product.name
  - "log_*.*"
review:
  order.notes: null
  "*.description": full_name
"#;
        let config = Config::parse(text).unwrap();
        assert_eq!(config.rule_for("order", "notes"), Some(&Rule::Null));
        assert_eq!(config.rule_for("product", "name"), Some(&Rule::Keep));
        assert_eq!(config.rule_for("log_2024", "payload"), Some(&Rule::Keep));
        assert!(matches!(
            config.rule_for("item", "description"),
            Some(Rule::Preset { name, .. }) if name == "full_name"
        ));
        assert_eq!(config.rule_for("user", "id"), None);
        assert_eq!(
            config.decisions_for("order", "notes"),
            vec![Decision::Review]
        );
        assert_eq!(config.decisions_for("user", "phone"), vec![Decision::Rules]);
    }

    #[test]
    fn legacy_review_list_is_accepted_without_rules() {
        let text =
            "version: 1\nsource: { kind: mysql }\nmode: anonymize\nreview:\n  - order.notes\n";
        let config = Config::parse(text).unwrap();
        assert_eq!(config.review, vec![("order.notes".to_string(), None)]);
        assert_eq!(config.rule_for("order", "notes"), None);
    }

    #[test]
    fn same_column_in_two_sections_is_rejected() {
        let base = "version: 1\nsource: { kind: mysql }\nmode: anonymize\n";
        for extra in [
            "tables:\n  product:\n    name: keep\nexclude:\n  - product.name\n",
            "review:\n  product.name: full_name\nexclude:\n  - product.name\n",
            "columns:\n  \"*.notes\": null\nreview:\n  \"*.notes\": null\n",
        ] {
            let err = Config::parse(&format!("{base}{extra}")).unwrap_err();
            assert!(err.to_string().contains("in both"), "{err}");
        }
        // Overlapping globs are only detectable against a schema.
        let text = format!("{base}columns:\n  order.notes: null\nexclude:\n  - \"*.notes\"\n");
        let config = Config::parse(&text).unwrap();
        assert_eq!(
            config.decisions_for("order", "notes"),
            vec![Decision::Rules, Decision::Exclude]
        );
    }

    #[test]
    fn rejects_bad_patterns() {
        let base = "version: 1\nsource: { kind: mysql }\nmode: anonymize\n";
        let err = Config::parse(&format!("{base}columns:\n  description: null\n")).unwrap_err();
        assert!(err.to_string().contains("<table>.<column>"), "{err}");
        let err = Config::parse(&format!("{base}columns:\n  \"*.mail\": emial\n")).unwrap_err();
        assert!(err.to_string().contains("email"), "{err}");
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
