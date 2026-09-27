//! Transformation engine: applies the config to an event stream.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
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
    /// Columns covered by a single-column `PRIMARY KEY`/`UNIQUE KEY` constraint:
    /// their preset-generated values must never collide.
    unique_columns: HashSet<String>,
    /// Per unique column: fingerprint of the original value -> selected attempt
    /// number (so the same input always yields the same output, see
    /// `resolve_unique_preset_value`).
    dedup_by_input: HashMap<String, HashMap<u128, u32>>,
    /// Per unique column: fingerprints (64-bit, see [`short_fingerprint`]) of
    /// outputs already assigned, to detect collisions between distinct inputs.
    used_outputs: HashMap<String, HashSet<u64>>,
}

/// 128-bit fingerprint of a value, for the dedup state of `UNIQUE` columns:
/// the values themselves (original or generated) are never stored, only ~50
/// bytes per entry instead of ~300.
///
/// Two independent SipHash-1-3 hashes (raw input / prefixed input). An
/// accidental collision is on the order of 2^-128; should one occur, the
/// worst case is a needlessly drawn output variant.
fn fingerprint(bytes: &[u8]) -> u128 {
    let mut b = DefaultHasher::new();
    0xFFu8.hash(&mut b);
    bytes.hash(&mut b);
    ((short_fingerprint(bytes) as u128) << 64) | b.finish() as u128
}

/// 64-bit fingerprint, sufficient for the set of assigned outputs: a false
/// positive only costs a needless variant (never a duplicate), and the set
/// takes 8 bytes per entry.
fn short_fingerprint(bytes: &[u8]) -> u64 {
    let mut a = DefaultHasher::new();
    bytes.hash(&mut a);
    a.finish()
}

/// Derives the HMAC key used for the whole transformation.
///
/// In `anonymize` mode, a random 32-byte key generated once at startup and
/// never persisted. In `pseudonymize` mode, the bytes of `$SOSIE_KEY`
/// (already validated by `Config::load`).
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
            // "Set NULL (or empty string if NOT NULL, detected from the schema)": a
            // literal `NULL` in a `NOT NULL` column would make the SQL dump fail on
            // import.
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
                // GENERATED columns never appear in an INSERT (mysqldump omits them):
                // they can't and needn't be covered.
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

/// Maximum number of retries before giving up and returning a value anyway
/// (should never be reached in practice: the preset's output space would be
/// near saturation by then).
const MAX_DEDUP_ATTEMPTS: u32 = 1000;

/// Computes a preset's output for a `UNIQUE` column, guaranteeing it never
/// collides with an output already assigned to a DIFFERENT original value in
/// the same column:
/// - input already seen -> recompute exactly the same output (the stored
///   attempt number is enough, since presets are deterministic);
/// - new input -> compute normally, and only on an actual collision with
///   another input, draw a variant (seed perturbed by an attempt number)
///   until a free one is found.
///
/// The state only holds fingerprints (see [`fingerprint`]): memory stays
/// bounded even with tens of millions of unique values.
#[allow(clippy::too_many_arguments)]
fn resolve_unique_preset_value(
    preset: &dyn Preset,
    preset_name: &str,
    raw: &[u8],
    key: &[u8],
    max_len: Option<u32>,
    dedup_map: &mut HashMap<u128, u32>,
    used_outputs: &mut HashSet<u64>,
    report: &mut crate::report::ColumnReport,
) -> Vec<u8> {
    let compute = |attempt: u32, report: &mut crate::report::ColumnReport| -> Option<Vec<u8>> {
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
        match out {
            Value::Str(s) => {
                let bytes = s.into_owned();
                Some(match max_len {
                    Some(max) => truncate_to_chars(&bytes, max as usize, report),
                    None => bytes,
                })
            }
            // A preset applied to a `Str` must return a `Str`; otherwise there is
            // nothing to dedup.
            Value::Raw(b) => {
                let _ = b;
                None
            }
            Value::Null => None,
        }
    };

    let input_fp = fingerprint(raw);
    if let Some(&attempt) = dedup_map.get(&input_fp) {
        // Already counted in the report the first time: throwaway report.
        let mut scratch = crate::report::ColumnReport::default();
        return compute(attempt, &mut scratch).unwrap_or_default();
    }

    let mut candidate = Vec::new();
    let mut chosen = 0;
    for attempt in 0..=MAX_DEDUP_ATTEMPTS {
        let Some(bytes) = compute(attempt, report) else {
            dedup_map.insert(input_fp, attempt);
            return Vec::new();
        };
        chosen = attempt;
        let free = used_outputs.insert(short_fingerprint(&bytes));
        candidate = bytes;
        if free {
            break;
        }
    }

    dedup_map.insert(input_fp, chosen);
    candidate
}

/// Dedup state of a `UNIQUE` column: (input -> assigned output, set of
/// assigned outputs).
type UniqueDedupState<'a> = (&'a mut HashMap<u128, u32>, &'a mut HashSet<u64>);

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

/// Transformation progress reported to the caller (progress bar, log…).
/// Never carries a data value.
#[derive(Debug, Clone, Copy)]
pub enum Progress<'a> {
    /// A `CREATE TABLE` was just read: entering this table.
    Table(&'a str),
    /// Total rows emitted so far (sent every [`PROGRESS_EVERY_ROWS`] rows and
    /// at the end of each `INSERT`).
    Rows(u64),
}

/// Interval (in rows) between [`Progress::Rows`] notifications.
pub const PROGRESS_EVERY_ROWS: u64 = 2_048;

/// Runs the full transformation of a `mysqldump` dump, from the input
/// stream to the output stream, according to `config`.
pub fn run<R: Read, W: Write>(
    config: &Config,
    reader: R,
    writer: W,
    report: &mut Report,
) -> Result<()> {
    run_with_progress(config, reader, writer, report, |_| {})
}

/// Like [`run`], calling `on_progress` along the way.
pub fn run_with_progress<R: Read, W: Write, F: FnMut(Progress<'_>)>(
    config: &Config,
    reader: R,
    writer: W,
    report: &mut Report,
    mut on_progress: F,
) -> Result<()> {
    let key = resolve_key(config)?;
    let mut parser = MysqlParser::new(reader);
    let mut out = MysqlWriter::new(writer);

    let mut plan: Option<TablePlan> = None;
    let mut current_order: Vec<String> = Vec::new();
    let mut skipping = false;
    let mut rows_total: u64 = 0;

    while let Some(event) = parser.next_event()? {
        match event {
            Event::TableSchema(ref table) => {
                on_progress(Progress::Table(&table.name));
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
                if skipping {
                    // Neither written nor parsed: jump straight to the next statement.
                    parser.skip_rows();
                    continue;
                }
                current_order = columns.clone().unwrap_or_else(|| p.schema_order.clone());
                out.write_event(&event)?;
            }
            Event::Row(ref values) => {
                if skipping {
                    continue;
                }
                let table_name = plan.as_ref().expect("Row sans RowsBegin").table.clone();
                let mut transformed: Vec<Value> = Vec::with_capacity(values.len());
                for (v, col_name) in values.iter().zip(current_order.iter()) {
                    let p = plan.as_mut().expect("Row sans RowsBegin");
                    // Disjoint borrows of the same `&mut TablePlan`: `column_action` (read) on
                    // one side, `dedup_by_input`/`used_outputs` (mutated) on the other. Distinct
                    // fields, so the compiler splits the borrows.
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
                rows_total += 1;
                if rows_total.is_multiple_of(PROGRESS_EVERY_ROWS) {
                    on_progress(Progress::Rows(rows_total));
                }
            }
            Event::RowsEnd => {
                on_progress(Progress::Rows(rows_total));
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
        // password is always replaced by the configured constant.
        assert!(text.contains("$2y$13$DEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVONLYDEVO"));
        // audit_log is in skip_tables: structure kept, zero rows.
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
        // `user.password` is `NOT NULL` in the schema: a literal `NULL` would
        // produce a dump that fails on import.
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
        // The password column (7th value) must be an empty string, never NULL.
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

        // Fake preset with a tiny output space (10 possible values), to force
        // collisions and check that the fallback resolves them instead of letting
        // a duplicate through.
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
        let mut first_for_input_3 = Vec::new();

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
            if i == 3 {
                first_for_input_3 = out.clone();
            }
            outputs.insert(out);
        }

        assert_eq!(
            outputs.len(),
            10,
            "10 entrées distinctes doivent occuper les 10 sorties possibles du pool, sans collision"
        );

        // A previously seen input must always yield exactly the same output.
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
        assert_eq!(again, first_for_input_3);
        assert_eq!(
            used.len(),
            10,
            "une entrée répétée ne consomme pas de nouvelle sortie"
        );
    }
}
