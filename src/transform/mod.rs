//! Moteur de transformation : applique la config à un flux d'événements.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use anyhow::{Context, Result, bail};

use crate::config::{Config, Mode, OnUnclassified, Rule};
use crate::dump::mysql::{MysqlParser, MysqlWriter};
use crate::dump::{DumpParser, DumpWriter, Event, Table as DumpTable, Value};
use crate::presets::{self, ColumnCtx, Preset};
use crate::report::Report;
use crate::scan;

enum Action {
    Keep,
    Null,
    Constant(String),
    Preset(Box<dyn Preset>, String),
}

struct TablePlan {
    table: String,
    skip: bool,
    schema_order: Vec<String>,
    column_action: HashMap<String, Action>,
    max_len: HashMap<String, Option<u32>>,
    /// Colonnes couvertes par une contrainte `PRIMARY KEY`/`UNIQUE KEY` mono-colonne :
    /// leurs valeurs générées par preset ne doivent jamais entrer en collision.
    unique_columns: HashSet<String>,
    /// Par colonne unique : valeur d'origine -> valeur déjà attribuée (pour
    /// qu'une même entrée redonne toujours la même sortie, cf. `dedup_by_input`).
    dedup_by_input: HashMap<String, HashMap<Vec<u8>, Vec<u8>>>,
    /// Par colonne unique : ensemble des sorties déjà attribuées, pour
    /// détecter une collision entre deux entrées différentes.
    used_outputs: HashMap<String, HashSet<Vec<u8>>>,
}

/// Dérive la clé HMAC à utiliser pour toute la transformation.
///
/// En `anonymize`, une clé aléatoire de 32 octets, générée une fois au
/// démarrage et jamais conservée. En `pseudonymize`, les octets de
/// `$SOSIE_KEY` (déjà validée par `Config::load`).
pub fn resolve_key(config: &Config) -> Result<Vec<u8>> {
    match config.mode {
        Mode::Anonymize => Ok(rand::random::<[u8; 32]>().to_vec()),
        Mode::Pseudonymize => {
            let key = std::env::var("SOSIE_KEY").context("SOSIE_KEY manquante")?;
            Ok(key.into_bytes())
        }
    }
}

fn build_table_plan(config: &Config, table: &DumpTable) -> Result<TablePlan> {
    let skip = config.skip_tables.iter().any(|t| t == &table.name)
        || config.truncate_tables.iter().any(|t| t == &table.name);
    let rules = config.tables.get(&table.name);

    let mut column_action = HashMap::new();
    let mut max_len = HashMap::new();
    let mut unique_columns = HashSet::new();

    for col in &table.columns {
        max_len.insert(col.name.clone(), col.max_len);
        if col.unique {
            unique_columns.insert(col.name.clone());
        }
        let rule = rules.and_then(|m| m.get(&col.name));
        let action = match rule {
            Some(Rule::Keep) => Action::Keep,
            // "Met NULL (ou chaîne vide si NOT NULL, détecté au schéma)" — un
            // `NULL` littéral sur une colonne `NOT NULL` produirait un dump
            // SQL invalide à l'import.
            Some(Rule::Null) => {
                if col.nullable {
                    Action::Null
                } else {
                    Action::Constant(String::new())
                }
            }
            Some(Rule::Constant(s)) => Action::Constant(s.clone()),
            Some(Rule::Preset { name, params }) => {
                Action::Preset(presets::build(name, params)?, name.clone())
            }
            None => {
                // Une colonne GENERATED n'apparaît jamais dans un INSERT
                // (mysqldump l'omet) : impossible et inutile de la couvrir.
                if !skip
                    && !col.generated
                    && config.defaults.on_unclassified == OnUnclassified::Fail
                {
                    let classification = scan::classify_by_name(&col.name);
                    if classification.score >= scan::THRESHOLD_MANDATORY {
                        bail!(
                            "{}.{} : colonne détectée comme sensible ({}) sans règle dans la config. \
                             Ajoute une règle explicite, ou passe `defaults.on_unclassified: keep` \
                             si c'est un faux positif.",
                            table.name,
                            col.name,
                            classification.reason
                        );
                    }
                }
                Action::Keep
            }
        };
        column_action.insert(col.name.clone(), action);
    }

    Ok(TablePlan {
        table: table.name.clone(),
        skip,
        schema_order: table.columns.iter().map(|c| c.name.clone()).collect(),
        column_action,
        max_len,
        unique_columns,
        dedup_by_input: HashMap::new(),
        used_outputs: HashMap::new(),
    })
}

fn truncate_to_chars(
    bytes: &[u8],
    max: usize,
    report: &mut crate::report::ColumnReport,
) -> Vec<u8> {
    let s = String::from_utf8_lossy(bytes);
    if s.chars().count() > max {
        report.truncated += 1;
        s.chars().take(max).collect::<String>().into_bytes()
    } else {
        s.into_owned().into_bytes()
    }
}

/// Nombre de secondes tentatives avant d'abandonner et de renvoyer quand même
/// une valeur (ne devrait jamais être atteint en pratique : à ce stade
/// l'espace de sortie du preset serait de toute façon proche de la saturation).
const MAX_DEDUP_ATTEMPTS: u32 = 1000;

/// Calcule la sortie d'un preset pour une colonne `UNIQUE`, en garantissant
/// qu'elle ne collisionne jamais avec une sortie déjà attribuée à une AUTRE
/// valeur d'origine dans cette même colonne :
/// - même entrée déjà vue -> on renvoie exactement la même sortie qu'avant
///   (préserve la cohérence habituelle, ex. deux lignes avec le même email) ;
/// - entrée nouvelle -> on calcule normalement, et seulement en cas de
///   collision réelle avec une autre entrée on retire une variante (graine
///   perturbée par un numéro de tentative) jusqu'à en trouver une libre.
#[allow(clippy::too_many_arguments)]
fn resolve_unique_preset_value(
    preset: &dyn Preset,
    preset_name: &str,
    raw: &[u8],
    key: &[u8],
    max_len: Option<u32>,
    dedup_map: &mut HashMap<Vec<u8>, Vec<u8>>,
    used_outputs: &mut HashSet<Vec<u8>>,
    report: &mut crate::report::ColumnReport,
) -> Vec<u8> {
    if let Some(existing) = dedup_map.get(raw) {
        return existing.clone();
    }

    let mut candidate = Vec::new();
    for attempt in 0..=MAX_DEDUP_ATTEMPTS {
        let seed_name = if attempt == 0 {
            Cow::Borrowed(preset_name)
        } else {
            Cow::Owned(format!("{preset_name}#{attempt}"))
        };
        let seed = presets::seed_for(key, &seed_name, raw);
        let out = preset.apply(
            &Value::Str(Cow::Borrowed(raw)),
            &seed,
            &ColumnCtx { max_len },
        );
        let mut bytes = match out {
            Value::Str(s) => s.into_owned(),
            other => {
                // Un preset appliqué à un `Str` doit renvoyer un `Str` ; s'il
                // ne le fait pas, on n'a rien à dédupliquer.
                dedup_map.insert(raw.to_vec(), Vec::new());
                return match other {
                    Value::Raw(b) => b.to_vec(),
                    _ => Vec::new(),
                };
            }
        };
        if let Some(max) = max_len {
            bytes = truncate_to_chars(&bytes, max as usize, report);
        }
        if used_outputs.insert(bytes.clone()) {
            candidate = bytes;
            break;
        }
        candidate = bytes;
    }

    dedup_map.insert(raw.to_vec(), candidate.clone());
    candidate
}

/// État de déduplication d'une colonne `UNIQUE` : (entrée -> sortie déjà
/// attribuée, ensemble des sorties déjà attribuées).
type UniqueDedupState<'a> = (&'a mut HashMap<Vec<u8>, Vec<u8>>, &'a mut HashSet<Vec<u8>>);

#[allow(clippy::too_many_arguments)]
fn apply_action<'a>(
    action: &Action,
    value: Value<'a>,
    key: &[u8],
    max_len: Option<u32>,
    unique: Option<UniqueDedupState<'_>>,
    report: &mut crate::report::ColumnReport,
) -> Value<'a> {
    match action {
        Action::Keep => {
            report.kept += 1;
            value
        }
        Action::Null => {
            report.null += 1;
            Value::Null
        }
        Action::Constant(s) => match value {
            Value::Null => {
                report.null += 1;
                Value::Null
            }
            _ => {
                report.transformed += 1;
                Value::Str(Cow::Owned(s.clone().into_bytes()))
            }
        },
        Action::Preset(preset, name) => match &value {
            Value::Null => {
                report.null += 1;
                Value::Null
            }
            Value::Str(s) if s.is_empty() => {
                report.kept += 1;
                value
            }
            Value::Raw(_) => {
                report.kept += 1;
                value
            }
            Value::Str(s) => {
                report.transformed += 1;
                if let Some((dedup_map, used_outputs)) = unique {
                    let bytes = resolve_unique_preset_value(
                        preset.as_ref(),
                        name,
                        s,
                        key,
                        max_len,
                        dedup_map,
                        used_outputs,
                        report,
                    );
                    Value::Str(Cow::Owned(bytes))
                } else {
                    let seed = presets::seed_for(key, name, s);
                    let ctx = ColumnCtx { max_len };
                    let out = preset.apply(&value, &seed, &ctx);
                    match (max_len, out) {
                        (Some(max), Value::Str(cow)) => {
                            Value::Str(Cow::Owned(truncate_to_chars(&cow, max as usize, report)))
                        }
                        (_, other) => other,
                    }
                }
            }
        },
    }
}

/// Exécute la transformation complète d'un dump `mysqldump`, du flux d'entrée
/// vers le flux de sortie, en suivant `config`.
pub fn run<R: Read, W: Write>(
    config: &Config,
    reader: R,
    writer: W,
    report: &mut Report,
) -> Result<()> {
    let key = resolve_key(config)?;
    let mut parser = MysqlParser::new(reader);
    let mut out = MysqlWriter::new(writer);

    let mut plan: Option<TablePlan> = None;
    let mut current_order: Vec<String> = Vec::new();
    let mut skipping = false;

    while let Some(event) = parser.next_event()? {
        match event {
            Event::TableSchema(ref table) => {
                let new_plan = build_table_plan(config, table)?;
                report.table_mut(&table.name).skipped = new_plan.skip;
                plan = Some(new_plan);
                out.write_event(&event)?;
            }
            Event::RowsBegin {
                ref table,
                ref columns,
                ..
            } => {
                let p = plan
                    .as_ref()
                    .filter(|p| &p.table == table)
                    .with_context(|| format!("INSERT INTO {table} sans CREATE TABLE préalable"))?;
                skipping = p.skip;
                current_order = columns.clone().unwrap_or_else(|| p.schema_order.clone());
                if !skipping {
                    out.write_event(&event)?;
                }
            }
            Event::Row(ref values) => {
                if skipping {
                    continue;
                }
                let table_name = plan.as_ref().expect("Row sans RowsBegin").table.clone();
                let mut transformed: Vec<Value> = Vec::with_capacity(values.len());
                for (v, col_name) in values.iter().zip(current_order.iter()) {
                    let p = plan.as_mut().expect("Row sans RowsBegin");
                    // Emprunts disjoints d'un même `&mut TablePlan` : `column_action`
                    // (lu) d'un côté, `dedup_by_input`/`used_outputs` (mutés) de
                    // l'autre — deux champs distincts, le compilateur les sépare.
                    let action = p.column_action.get(col_name).unwrap_or(&Action::Keep);
                    let max_len = p.max_len.get(col_name).copied().flatten();
                    let unique = if p.unique_columns.contains(col_name) {
                        Some((
                            p.dedup_by_input.entry(col_name.clone()).or_default(),
                            p.used_outputs.entry(col_name.clone()).or_default(),
                        ))
                    } else {
                        None
                    };
                    let col_report = report
                        .table_mut(&table_name)
                        .columns
                        .entry(col_name.clone())
                        .or_default();
                    transformed.push(apply_action(
                        action,
                        v.clone(),
                        &key,
                        max_len,
                        unique,
                        col_report,
                    ));
                }
                let tr = report.table_mut(&table_name);
                tr.rows_in += 1;
                tr.rows_out += 1;
                out.write_event(&Event::Row(transformed))?;
            }
            Event::RowsEnd => {
                if !skipping {
                    out.write_event(&event)?;
                }
            }
            Event::Raw(bytes) => {
                if is_notable_raw(bytes) {
                    report.raw_notable.push(first_line(bytes));
                }
                out.write_event(&Event::Raw(bytes))?;
            }
        }
    }

    Ok(())
}

fn is_notable_raw(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let upper = text.to_uppercase();
    upper.contains("CREATE")
        && (upper.contains("VIEW") || upper.contains("TRIGGER") || upper.contains("PROCEDURE"))
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(yaml: &str) -> Config {
        Config::parse(yaml).unwrap()
    }

    #[test]
    fn transforms_basic_fixture_deterministically_in_pseudonymize_mode() {
        unsafe {
            std::env::set_var("SOSIE_KEY", "test-key-do-not-use-in-prod-1234");
        }
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let cfg = config(include_str!(
            "../../fixtures/exemples/configs/03_pseudonymize.yaml"
        ));

        let mut out1 = Vec::new();
        let mut report1 = Report::new("pseudonymize");
        run(&cfg, input, &mut out1, &mut report1).unwrap();

        let mut out2 = Vec::new();
        let mut report2 = Report::new("pseudonymize");
        run(&cfg, input, &mut out2, &mut report2).unwrap();

        assert_eq!(
            out1, out2,
            "deux exécutions en pseudonymize doivent produire le même octet-à-octet"
        );
        assert!(!out1.is_empty());

        let text = String::from_utf8_lossy(&out1);
        assert!(!text.contains("jean.dupont@gmail.com"));
        assert!(text.contains("@example.org"));
        // password toujours remplacé par la constante configurée.
        assert!(text.contains("$2y$13$DEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVO"));
        // audit_log est dans skip_tables : structure gardée, zéro ligne.
        assert!(text.contains("CREATE TABLE `audit_log`"));
        assert!(!text.contains("INSERT INTO `audit_log`"));
    }

    #[test]
    fn refuses_to_run_when_a_sensitive_column_has_no_rule() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let cfg = config(include_str!(
            "../../fixtures/exemples/configs/02_incomplete.yaml"
        ));
        let mut out = Vec::new();
        let mut report = Report::new("anonymize");
        let err = run(&cfg, input, &mut out, &mut report).unwrap_err();
        assert!(err.to_string().contains("sans règle"));
    }

    #[test]
    fn null_rule_on_a_not_null_column_produces_empty_string_not_literal_null() {
        // `user.password` est `NOT NULL` dans le schéma : un `NULL` littéral
        // produirait un dump invalide à l'import.
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let yaml = r#"
version: 1
source: { kind: mysql }
mode: anonymize
defaults: { locale: fr_FR, on_unclassified: keep }
tables:
  user:
    password: null
"#;
        let cfg = config(yaml);
        let mut out = Vec::new();
        let mut report = Report::new("anonymize");
        run(&cfg, input, &mut out, &mut report).unwrap();
        let text = String::from_utf8_lossy(&out);
        // La colonne password (7e valeur) doit être une chaîne vide, jamais NULL.
        assert!(text.contains("1985-03-14',''"));
        assert!(!text.contains("1985-03-14',NULL"));
    }

    #[test]
    fn edge_cases_fixture_roundtrips_except_the_email_column() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/02_parser_edge_cases.sql");
        let cfg = config(include_str!(
            "../../fixtures/exemples/configs/02_parser_edge_cases.yaml"
        ));
        let mut out = Vec::new();
        let mut report = Report::new("anonymize");
        run(&cfg, input, &mut out, &mut report).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("x@y.fr"));
        assert!(text.contains("ligne1\\nligne2\\ttab\\\\backslash \\\"quoted\\\" \\0nul"));
    }

    #[test]
    fn dedup_with_fallback_fills_a_small_output_pool_without_collision() {
        use rand::RngExt;

        // Preset factice à espace de sortie minuscule (10 valeurs possibles),
        // pour déclencher des collisions à coup sûr et vérifier que le repli
        // les résout au lieu de laisser passer un doublon.
        struct TinyPoolPreset;
        impl Preset for TinyPoolPreset {
            fn apply<'a>(
                &self,
                _input: &Value<'a>,
                seed: &presets::Seed,
                _ctx: &ColumnCtx,
            ) -> Value<'a> {
                const POOL: [&str; 10] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
                let mut rng = seed.rng();
                let idx = rng.random_range(0..POOL.len());
                Value::Str(Cow::Owned(POOL[idx].as_bytes().to_vec()))
            }
        }

        let mut dedup_map = HashMap::new();
        let mut used = HashSet::new();
        let mut report = crate::report::ColumnReport::default();
        let mut outputs = HashSet::new();

        for i in 0..10 {
            let input = format!("input-{i}");
            let out = resolve_unique_preset_value(
                &TinyPoolPreset,
                "tiny",
                input.as_bytes(),
                b"key",
                None,
                &mut dedup_map,
                &mut used,
                &mut report,
            );
            outputs.insert(out);
        }

        assert_eq!(
            outputs.len(),
            10,
            "10 entrées distinctes doivent occuper les 10 sorties possibles du pool, sans collision"
        );

        // Une entrée déjà vue doit toujours redonner exactement la même sortie.
        let again = resolve_unique_preset_value(
            &TinyPoolPreset,
            "tiny",
            b"input-3",
            b"key",
            None,
            &mut dedup_map,
            &mut used,
            &mut report,
        );
        assert_eq!(again, dedup_map[b"input-3".as_slice()]);
    }
}
