//! Chargement et validation de `sosie.yaml`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};

/// Presets connus de la v0.1 (voir `src/presets`). Utilisé pour valider les
/// règles de la config et proposer une suggestion en cas de faute de frappe.
pub use crate::presets::KNOWN_PRESETS;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub version: u32,
    pub source: Source,
    pub mode: Mode,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub tables: BTreeMap<String, BTreeMap<String, Rule>>,
    #[serde(default)]
    pub skip_tables: Vec<String>,
    #[serde(default)]
    pub truncate_tables: Vec<String>,
    #[serde(default)]
    pub review: Vec<String>,
    #[serde(default)]
    pub sampling: Option<Sampling>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Anonymize,
    Pseudonymize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnclassified {
    #[default]
    Fail,
    Keep,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
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

/// Une règle de transformation pour une colonne : soit une forme courte
/// (`keep`, `null`, `constant("…")`, ou un nom de preset nu), soit une map
/// `{ preset: …, ...paramètres }`.
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
                    "null, une chaîne (\"email\", \"keep\", \"null\", `constant(\"...\")`) ou une map {{ preset: ..., ... }}"
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
            "`constant(...)` doit contenir une chaîne entre guillemets, trouvé : {s}"
        ));
    }
    Ok(Rule::Preset {
        name: s.to_string(),
        params: BTreeMap::new(),
    })
}

/// Clés qui n'ont rien à faire dans un fichier de config versionné.
///
/// On ne regarde que la racine et `source` : `tables.*.*` contient des noms
/// de colonnes définis par le schéma de l'application (une vraie table peut
/// très bien avoir une colonne `password` ou `key`), ce n'est pas de la
/// config sosie et ça ne doit jamais déclencher cette vérification.
const FORBIDDEN_KEYS: &[&str] = &["dsn", "password", "key"];

fn check_no_secrets(raw: &serde_yaml::Value) -> Result<()> {
    let Some(top) = raw.as_mapping() else {
        return Ok(());
    };
    for (k, v) in top {
        let Some(key) = k.as_str() else { continue };
        if FORBIDDEN_KEYS.contains(&key) {
            bail!(
                "clé interdite `{key}` à la racine de la config : ne mets jamais de secret (dsn/password/key) dedans, utilise une variable d'environnement"
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
                        "clé interdite `source.{skey}` : ne mets jamais de secret (dsn/password/key) dedans, utilise une variable d'environnement"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Distance de Levenshtein, pour suggérer un preset proche d'une faute de frappe.
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
            bail!("{table}.{column} : preset inconnu `{name}` (voulais-tu dire `{suggestion}` ?)")
        }
        None => bail!(
            "{table}.{column} : preset inconnu `{name}` (presets disponibles : {})",
            KNOWN_PRESETS.join(", ")
        ),
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Config> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config> {
        let raw: serde_yaml::Value = serde_yaml::from_str(text).context("YAML invalide")?;
        check_no_secrets(&raw)?;

        let config: Config = serde_yaml::from_value(raw).context("config invalide")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.source.kind != "mysql" {
            bail!(
                "source.kind `{}` non supporté en v0.1 (seul `mysql` l'est)",
                self.source.kind
            );
        }

        if self.defaults.locale != "fr_FR" {
            bail!(
                "defaults.locale `{}` non supportée en v0.1 (seule `fr_FR` l'est)",
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
                Ok(_) => bail!("SOSIE_KEY doit faire au moins 16 caractères"),
                Err(_) => bail!(
                    "mode: pseudonymize nécessite la variable d'environnement SOSIE_KEY (>= 16 caractères)"
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
            other => panic!("attendu Constant, trouvé {other:?}"),
        }
        match &config.tables["user"]["birth_date"] {
            Rule::Preset { name, params } => {
                assert_eq!(name, "date_shift");
                assert_eq!(params["days"], serde_yaml::Value::from(365));
            }
            other => panic!("attendu Preset, trouvé {other:?}"),
        }
    }

    #[test]
    fn loads_incomplete_fixture_without_erroring_at_load_time() {
        // 02_incomplete.yaml est syntaxiquement valide ; c'est `check` (étape 6)
        // qui doit détecter les colonnes manquantes, pas le chargement.
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
        // SOSIE_KEY absent dans l'environnement de test.
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
        // `tables.*.*` décrit le schéma réel de l'appli : une colonne nommée
        // `password` ou `key` est légitime et ne doit pas être bloquée.
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
