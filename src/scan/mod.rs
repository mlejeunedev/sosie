//! Sensitive column detection, by name and by content (`init`/`check`).

use std::collections::BTreeMap;
use std::io::Read;

use anyhow::Result;

use crate::dump::mysql::MysqlParser;
use crate::dump::{DumpParser, Event, SqlType, Value};
use crate::presets;

const MAX_SAMPLES: usize = 200;

pub const THRESHOLD_MANDATORY: f64 = 0.8;
pub const THRESHOLD_REVIEW: f64 = 0.4;

#[derive(Debug, Clone)]
pub struct Classification {
    pub score: f64,
    pub preset: Option<&'static str>,
    pub reason: String,
}

impl Classification {
    fn none() -> Self {
        Classification {
            score: 0.0,
            preset: None,
            reason: String::new(),
        }
    }

    pub fn is_mandatory(&self) -> bool {
        self.score >= THRESHOLD_MANDATORY
    }

    pub fn is_review(&self) -> bool {
        self.score >= THRESHOLD_REVIEW
    }
}

fn normalize(name: &str) -> String {
    // camelCase -> snake_case, then lowercase.
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// A column name split into lowercase words: `contactFirstName` ->
/// `["contact", "first", "name"]`. Patterns are matched against whole words, never
/// substrings: `tel` must not match `hotel`, nor `nom` match `nombre`.
struct Words(Vec<String>);

impl Words {
    fn of(column: &str) -> Self {
        Words(
            normalize(column)
                .split(['_', '-', ' ', '.'])
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// One of `patterns` appears as consecutive whole words: `"first_name"`
    /// matches `contact_first_name`, `"tel"` matches `tel_portable` but not `hotel`.
    fn has(&self, patterns: &[&str]) -> bool {
        patterns.iter().any(|p| {
            let seq: Vec<&str> = p.split('_').collect();
            self.0
                .windows(seq.len())
                .any(|w| w.iter().zip(&seq).all(|(a, b)| a == b))
        })
    }

    /// A word starts with one of `stems` (`birth` matches `birthdate`). Only for
    /// stems that never begin an unrelated word.
    fn has_prefix(&self, stems: &[&str]) -> bool {
        self.0
            .iter()
            .any(|w| stems.iter().any(|s| w.starts_with(s)))
    }

    /// A word ends with one of `stems` (`token` matches `accesstoken`).
    fn has_suffix(&self, stems: &[&str]) -> bool {
        self.0.iter().any(|w| stems.iter().any(|s| w.ends_with(s)))
    }

    fn last(&self) -> &str {
        self.0.last().map(String::as_str).unwrap_or("")
    }
}

/// Trailing words that turn a sensitive-sounding name into metadata about it:
/// `email_verified`, `address_id`, `phone_count`, `content_type`, `message_sent_at`.
const METADATA_SUFFIXES: &[&str] = &[
    "id", "ids", "uuid", "count", "type", "status", "enabled", "verified", "at",
];

/// Words that, right before `name`, denote a technical name rather than a
/// person's (`table_name`, `host_name`). Also excluded glued (`filename`).
const TECHNICAL_NAMES: &[&str] = &[
    "table", "file", "class", "role", "host", "route", "column", "field", "domain", "path", "dir",
    "method", "event", "template", "bucket", "queue", "job", "server", "database", "schema",
    "index", "module", "service", "package", "device", "browser", "type",
];

fn is_technical_name(words: &Words) -> bool {
    words.0.iter().enumerate().any(|(i, w)| {
        let glued = w
            .strip_suffix("name")
            .is_some_and(|stem| TECHNICAL_NAMES.contains(&stem));
        let split = w == "name" && i > 0 && TECHNICAL_NAMES.contains(&words.0[i - 1].as_str());
        glued || split
    })
}

fn sensitive(preset: &'static str, reason: &str) -> Classification {
    Classification {
        score: 0.9,
        preset: Some(preset),
        reason: reason.into(),
    }
}

/// Signal 1: detection by column name. See `docs/CAHIER_DES_CHARGES.md` §5.5.
pub fn classify_by_name(column: &str) -> Classification {
    let w = Words::of(column);

    if METADATA_SUFFIXES.contains(&w.last()) {
        return Classification::none();
    }

    if w.has(&["email", "mail", "courriel"]) || w.has_prefix(&["email", "courriel"]) {
        return sensitive("email", "name");
    }
    if w.has(&["first_name", "given_name", "prenom"]) || w.has_prefix(&["firstname", "prenom"]) {
        return sensitive("first_name", "name");
    }
    if w.has(&["last_name", "family_name", "surname", "nom"])
        || w.has_prefix(&["lastname", "surname"])
    {
        return sensitive("last_name", "name");
    }
    if w.has(&["phone", "tel", "mobile", "telephone"]) || w.has_prefix(&["phone", "telephone"]) {
        return sensitive("phone", "name");
    }
    if w.has(&["iban", "bban", "account_number"]) {
        return sensitive("iban", "name");
    }
    if w.has(&["bic", "swift"]) {
        return sensitive("bic", "name");
    }
    if w.has(&["ip_address", "ip_addr", "remote_addr"]) || w.last() == "ip" {
        return sensitive("ip", "name");
    }
    if w.has(&["address", "adresse", "street", "rue", "line1", "line2"])
        || w.has_prefix(&["address", "adresse"])
    {
        return sensitive("address_line", "name");
    }
    if w.has(&["city", "ville", "town"]) {
        return sensitive("city", "name");
    }
    if w.has(&["zip", "postcode", "postal_code", "code_postal"])
        || w.has_prefix(&["zipcode", "postcode", "postalcode"])
    {
        return sensitive("postcode", "name");
    }
    if w.has(&["dob"]) || w.has_prefix(&["birth", "naissance"]) {
        return sensitive("date_shift", "name (birth)");
    }
    if w.has(&["ssn", "nir", "secu", "social_security", "securite_sociale"]) {
        return sensitive("null", "name");
    }
    // No "secret"/"token" prefix: "secretary", "tokenizer".
    if w.has(&["passwd", "pwd", "salt", "api_key", "secrets", "tokens"])
        || w.has_prefix(&["password"])
        || w.has_suffix(&["password", "token", "secret"])
    {
        return sensitive("null", "name (token)");
    }
    if w.has(&["siret"]) {
        return sensitive("siret", "name");
    }
    if w.has(&["siren"]) {
        return sensitive("siren", "name");
    }
    // Not a VAT number when it is a rate or an amount ("vat_rate", "montant_tva").
    if w.has(&["vat", "tva"])
        && !w.has(&[
            "rate", "taux", "amount", "montant", "percent", "pct", "total", "ht", "ttc",
        ])
    {
        return sensitive("vat", "name");
    }
    if w.has(&["body", "content"]) || w.has_prefix(&["note", "comment", "message", "description"]) {
        return Classification {
            score: 0.5,
            preset: None,
            reason: "free text".into(),
        };
    }

    // Generic "name"/"holder" signal, combinable, with exclusions.
    if !is_technical_name(&w) {
        let mut score: f64 = 0.0;
        let mut hits = Vec::new();
        if w.has_suffix(&["name"]) {
            score += 0.5;
            hits.push("name");
        }
        if w.has_suffix(&["holder"]) {
            score += 0.5;
            hits.push("holder");
        }
        if !hits.is_empty() {
            return Classification {
                score: score.min(0.9),
                preset: Some("full_name"),
                reason: format!("name ({})", hits.join(" + ")),
            };
        }
    }

    Classification::none()
}

/// Signal 2: detection by content, on a sample of non-null values.
fn classify_by_content(sql_type: &SqlType, samples: &[Vec<u8>]) -> Classification {
    if samples.is_empty() {
        return Classification::none();
    }

    if *sql_type == SqlType::Json {
        const JSON_KEY_PATTERNS: &[&str] = &["email", "phone", "iban", "address", "ip_address"];
        let hit = samples.iter().any(|s| {
            let text = String::from_utf8_lossy(s);
            JSON_KEY_PATTERNS
                .iter()
                .any(|p| text.contains(&format!("\"{p}\"")))
        });
        if hit {
            return Classification {
                score: 0.5,
                preset: None,
                reason: "JSON containing a sensitive key".into(),
            };
        }
    }

    let texts: Vec<String> = samples
        .iter()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    let ratio = |pred: &dyn Fn(&str) -> bool| {
        texts.iter().filter(|t| pred(t)).count() as f64 / texts.len() as f64
    };

    let email_ratio = ratio(&|t: &str| is_email(t));
    if email_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("email"),
            reason: "content (email)".into(),
        };
    }
    let iban_ratio = ratio(&|t: &str| is_iban(t));
    if iban_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("iban"),
            reason: "content (IBAN)".into(),
        };
    }
    let siret_ratio = ratio(&|t: &str| is_luhn_number(t, 14));
    if siret_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("siret"),
            reason: "content (SIRET)".into(),
        };
    }
    let siren_ratio = ratio(&|t: &str| is_luhn_number(t, 9));
    if siren_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("siren"),
            reason: "content (SIREN)".into(),
        };
    }
    let vat_ratio = ratio(&|t: &str| {
        presets::vat_fr_is_valid(&t.chars().filter(|c| !c.is_whitespace()).collect::<String>())
    });
    if vat_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("vat"),
            reason: "content (VAT number)".into(),
        };
    }
    let phone_ratio = ratio(&|t: &str| is_phone(t));
    if phone_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("phone"),
            reason: "content (phone)".into(),
        };
    }
    let ip_ratio = ratio(&|t: &str| is_ip(t));
    if ip_ratio >= 0.8 {
        return Classification {
            score: 0.9,
            preset: Some("ip"),
            reason: "content (IP)".into(),
        };
    }

    let avg_len: f64 =
        texts.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / texts.len() as f64;
    if avg_len > 100.0 {
        let pii_ratio = ratio(&|t: &str| is_email(t) || is_phone(t));
        if pii_ratio >= 0.05 {
            return Classification {
                score: 0.5,
                preset: None,
                reason: "free text occasionally containing PII".into(),
            };
        }
    }

    Classification::none()
}

fn combine(name: Classification, content: Classification) -> Classification {
    if content.score > name.score {
        content
    } else {
        name
    }
}

pub fn is_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.contains(' ')
        && s.matches('@').count() == 1
}

pub fn is_ip(s: &str) -> bool {
    is_ipv4(s) || is_ipv6(s)
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.trim().split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty() && p.len() <= 3 && p.parse::<u16>().map(|n| n <= 255).unwrap_or(false)
        })
}

fn is_ipv6(s: &str) -> bool {
    // A real parse: "1:10" (a scale) or "12:30:00" (a time) are not addresses.
    s.trim().parse::<std::net::Ipv6Addr>().is_ok()
}

fn is_phone(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(rest) = compact.strip_prefix('+') {
        rest.len() >= 8 && rest.len() <= 15 && rest.chars().all(|c| c.is_ascii_digit())
    } else {
        compact.len() == 10
            && compact.starts_with('0')
            && compact.chars().all(|c| c.is_ascii_digit())
    }
}

/// `len` digits (spaces allowed) passing Luhn: SIREN (9) or SIRET (14).
fn is_luhn_number(s: &str, len: usize) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() == len && presets::luhn_is_valid(&compact)
}

fn is_iban(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = compact.chars().collect();
    if chars.len() < 15 || chars.len() > 34 {
        return false;
    }
    if !(chars[0].is_ascii_alphabetic() && chars[1].is_ascii_alphabetic()) {
        return false;
    }
    if !chars[2].is_ascii_digit() || !chars[3].is_ascii_digit() {
        return false;
    }
    if !chars[4..].iter().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    presets::iban_is_valid(&compact)
}

/// A schema column with its content sample (empty if only the schema was
/// scanned, without data).
pub struct ScannedColumn {
    pub name: String,
    pub sql_type: SqlType,
    pub generated: bool,
    pub samples: Vec<Vec<u8>>,
}

pub struct ScannedTable {
    pub name: String,
    pub columns: Vec<ScannedColumn>,
}

/// Replays a full dump and builds the list of tables, with a sample (up to
/// 200 non-null values) per text column.
pub fn scan_dump<R: Read>(reader: R) -> Result<Vec<ScannedTable>> {
    let mut parser = MysqlParser::new(reader);
    let mut tables: Vec<ScannedTable> = Vec::new();
    let mut current: Option<(usize, Vec<usize>)> = None; // (table index, column index by Row position)

    while let Some(event) = parser.next_event()? {
        match event {
            Event::TableSchema(table) => {
                tables.push(ScannedTable {
                    name: table.name.clone(),
                    columns: table
                        .columns
                        .iter()
                        .map(|c| ScannedColumn {
                            name: c.name.clone(),
                            sql_type: c.sql_type.clone(),
                            generated: c.generated,
                            samples: Vec::new(),
                        })
                        .collect(),
                });
            }
            Event::RowsBegin { table, columns, .. } => {
                let idx = tables.iter().position(|t| t.name == table);
                current = idx.map(|ti| {
                    let positions = match &columns {
                        Some(names) => names
                            .iter()
                            .map(|n| {
                                tables[ti]
                                    .columns
                                    .iter()
                                    .position(|c| &c.name == n)
                                    .unwrap_or(usize::MAX)
                            })
                            .collect(),
                        None => (0..tables[ti].columns.len()).collect(),
                    };
                    (ti, positions)
                });
            }
            Event::Row(values) => {
                if let Some((ti, positions)) = &current {
                    for (i, value) in values.iter().enumerate() {
                        let Some(&col_idx) = positions.get(i) else {
                            continue;
                        };
                        if col_idx == usize::MAX {
                            continue;
                        }
                        if let Value::Str(s) = value {
                            let col = &mut tables[*ti].columns[col_idx];
                            if col.samples.len() < MAX_SAMPLES && !s.is_empty() {
                                col.samples.push(s.to_vec());
                            }
                        }
                    }
                }
            }
            Event::RowsEnd => current = None,
            Event::Raw(_) => {}
        }
    }

    Ok(tables)
}

/// Classifies every column of a scan. Returns `table.column -> Classification`.
pub fn classify(tables: &[ScannedTable]) -> BTreeMap<(String, String), Classification> {
    let mut out = BTreeMap::new();
    for table in tables {
        for column in &table.columns {
            if column.generated {
                // GENERATED columns never appear in an INSERT (mysqldump omits them):
                // they can't and needn't be classified.
                continue;
            }
            let name_score = classify_by_name(&column.name);
            let content_score = classify_by_content(&column.sql_type, &column.samples);
            out.insert(
                (table.name.clone(), column.name.clone()),
                combine(name_score, content_score),
            );
        }
    }
    out
}

/// Columns requiring an explicit rule (`score >= 0.8`) but lacking one.
pub fn missing_mandatory(
    classifications: &BTreeMap<(String, String), Classification>,
    has_rule: impl Fn(&str, &str) -> bool,
    is_exempt_table: impl Fn(&str) -> bool,
) -> Vec<(&str, &str, &Classification)> {
    classifications
        .iter()
        .filter(|((table, _), c)| c.is_mandatory() && !is_exempt_table(table))
        .filter(|((table, column), _)| !has_rule(table, column))
        .map(|((table, column), c)| (table.as_str(), column.as_str(), c))
        .collect()
}

/// Gray-zone columns (`0.4 <= score < 0.8`) without an explicit rule.
pub fn needs_review(
    classifications: &BTreeMap<(String, String), Classification>,
    has_rule: impl Fn(&str, &str) -> bool,
    is_exempt_table: impl Fn(&str) -> bool,
) -> Vec<(&str, &str, &Classification)> {
    classifications
        .iter()
        .filter(|((table, _), c)| c.is_review() && !c.is_mandatory() && !is_exempt_table(table))
        .filter(|((table, column), _)| !has_rule(table, column))
        .map(|((table, column), c)| (table.as_str(), column.as_str(), c))
        .collect()
}

/// A column name with the same rule in at least this many tables is written once,
/// as a `"*.<name>"` pattern.
const PATTERN_MIN_TABLES: usize = 3;

/// A starter config produced by `init`, with counters for the summary.
pub struct InitConfig {
    pub yaml: String,
    /// Confirmed rules: `tables` entries and `columns` patterns.
    pub rules: usize,
    /// Entries of `review:` (default rules awaiting a human decision).
    pub to_review: usize,
}

/// A rule proposed by `init` for one column.
struct Proposal<'a> {
    table: &'a str,
    column: &'a str,
    rule: String,
    /// Gray zone (`0.4..0.8`): goes to `review:` instead of the rules.
    review: bool,
    reason: &'a str,
}

/// One line of the generated config: a `"<table>.<column>"` key (possibly a
/// `*.<column>` pattern) with its rule.
struct Line {
    key: String,
    rule: String,
    comment: String,
}

fn yaml_key(key: &str) -> String {
    if key.starts_with('*') {
        format!("\"{key}\"")
    } else {
        key.to_string()
    }
}

/// Generates a starter `sosie.yaml`. Columns `>= 0.8` get their suggested
/// preset under `tables` (or a `columns` pattern). Gray-zone columns
/// (`0.4..0.8`) go to `review:` only, with a cautious default rule (the
/// suggested preset, else `null`): the human moves each line to `columns` to
/// confirm it, or its key to `exclude` to keep the column as is. A column name
/// sharing the same rule in every table that has it (at least 3) becomes a
/// single `*.<name>` pattern. Written by hand rather than through serde to
/// preserve the comments.
pub fn render_init_yaml(tables: &[ScannedTable]) -> InitConfig {
    let classifications = classify(tables);
    let proposals: Vec<Proposal> = classifications
        .iter()
        .filter(|(_, c)| c.is_review())
        .map(|((table, column), c)| Proposal {
            table,
            column,
            rule: match c.preset {
                Some("date_shift") => "{ preset: date_shift, days: 365 }".to_string(),
                Some(preset) => preset.to_string(),
                None => "null".to_string(),
            },
            review: !c.is_mandatory(),
            reason: &c.reason,
        })
        .collect();

    // A `*.<name>` pattern also matches the columns of that name it wasn't built
    // from: only group when it covers every column of that name in the dump.
    let mut name_count: BTreeMap<&str, usize> = BTreeMap::new();
    for column in tables.iter().flat_map(|t| &t.columns) {
        if !column.generated {
            *name_count.entry(column.name.as_str()).or_default() += 1;
        }
    }
    let mut groups: BTreeMap<(&str, &str, bool), Vec<&Proposal>> = BTreeMap::new();
    for p in &proposals {
        groups
            .entry((p.column, p.rule.as_str(), p.review))
            .or_default()
            .push(p);
    }
    let grouped: BTreeMap<&str, (&str, bool, usize)> = groups
        .iter()
        .filter(|((column, _, _), members)| {
            !column.contains(['*', '.', '"', '\\'])
                && members.len() >= PATTERN_MIN_TABLES
                && members.len() == name_count[column]
        })
        .map(|((column, rule, review), members)| (*column, (*rule, *review, members.len())))
        .collect();

    let mut exact: BTreeMap<&str, Vec<&Proposal>> = BTreeMap::new();
    let mut patterns: Vec<Line> = Vec::new();
    let mut review: Vec<Line> = Vec::new();
    for p in &proposals {
        if grouped.contains_key(p.column) {
            continue;
        }
        if p.review {
            review.push(Line {
                key: format!("{}.{}", p.table, p.column),
                rule: p.rule.clone(),
                comment: p.reason.to_string(),
            });
        } else {
            exact.entry(p.table).or_default().push(p);
        }
    }
    for (column, &(rule, is_review, count)) in &grouped {
        let mut reasons: Vec<&str> = groups[&(*column, rule, is_review)]
            .iter()
            .map(|p| p.reason)
            .collect();
        reasons.sort();
        reasons.dedup();
        let line = Line {
            key: format!("*.{column}"),
            rule: rule.to_string(),
            comment: format!("{} ({count} tables)", reasons.join(", ")),
        };
        if is_review {
            review.push(line);
        } else {
            patterns.push(line);
        }
    }

    let mut out = String::new();
    out.push_str("version: 1\n\n");
    out.push_str("source:\n  kind: mysql\n\n");
    out.push_str("mode: anonymize\n\n");
    out.push_str("defaults:\n  locale: fr_FR\n  on_unclassified: fail\n\n");

    out.push_str("# Columns detected with confidence, with their rule.\n");
    if exact.is_empty() {
        out.push_str("tables: {}\n");
    } else {
        out.push_str("tables:\n");
    }
    for (table, columns) in &exact {
        out.push_str(&format!("  {table}:\n"));
        for p in columns {
            out.push_str(&format!("    {}: {}  # {}\n", p.column, p.rule, p.reason));
        }
    }
    out.push('\n');

    out.push_str(
        "# One rule for several columns (`*` = anything). Confirmed `review` lines go here.\n",
    );
    if patterns.is_empty() {
        out.push_str("columns: {}\n");
    } else {
        out.push_str("columns:\n");
        for line in &patterns {
            out.push_str(&format!(
                "  {}: {}  # {}\n",
                yaml_key(&line.key),
                line.rule,
                line.comment
            ));
        }
    }
    out.push('\n');

    out.push_str(
        "# Columns kept as is, never anonymized (false positives, harmless data).\n\
         # Example: - products.productName\n",
    );
    out.push_str("exclude: []\n\n");

    out.push_str(
        "# TO DECIDE: columns that may hold personal data, with a cautious default rule\n\
         # (applied as long as they stay here). For each line, either move it to `columns:`\n\
         # to confirm the rule (or change it), or move its key to `exclude:` to keep the\n\
         # column as is. `sosie check` fails until this section is empty.\n",
    );
    if review.is_empty() {
        out.push_str("review: {}\n");
    } else {
        out.push_str("review:\n");
        for line in &review {
            out.push_str(&format!(
                "  {}: {}  # {}\n",
                yaml_key(&line.key),
                line.rule,
                line.comment
            ));
        }
    }

    InitConfig {
        yaml: out,
        rules: exact.values().map(Vec::len).sum::<usize>() + patterns.len(),
        to_review: review.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn classifies_basic_fixture_columns() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let tables = scan_dump(input).unwrap();
        let classifications = classify(&tables);

        let get = |t: &str, c: &str| {
            classifications
                .get(&(t.to_string(), c.to_string()))
                .cloned()
                .unwrap()
        };

        assert_eq!(get("user", "email").preset, Some("email"));
        assert!(get("user", "email").is_mandatory());

        assert_eq!(get("user", "first_name").preset, Some("first_name"));
        assert!(get("user", "first_name").is_mandatory());

        assert_eq!(get("bank_account", "holder_name").preset, Some("full_name"));
        assert!(get("bank_account", "holder_name").is_mandatory());

        assert!(get("user", "nickname").is_review());
        assert!(!get("user", "nickname").is_mandatory());

        assert!(get("product", "name").is_review());
        assert!(!get("product", "name").is_mandatory());

        assert!(get("order", "notes").is_review());

        assert!(get("audit_log", "payload").is_review());

        assert!(get("audit_log", "ip_address").is_mandatory());
        assert_eq!(get("audit_log", "ip_address").preset, Some("ip"));

        assert!(get("bank_account", "iban").is_mandatory());
        assert_eq!(get("bank_account", "iban").preset, Some("iban"));

        // A clearly neutral column must not trigger any signal.
        assert_eq!(get("product", "price").score, 0.0);
        assert_eq!(get("user", "id").score, 0.0);
    }

    #[test]
    fn company_identifiers_by_name() {
        assert_eq!(classify_by_name("siret").preset, Some("siret"));
        assert_eq!(classify_by_name("companySiren").preset, Some("siren"));
        assert_eq!(classify_by_name("vat_number").preset, Some("vat"));
        assert_eq!(classify_by_name("numero_tva").preset, Some("vat"));
        assert!(classify_by_name("siret").is_mandatory());
        // Not a VAT number: a substring, or a rate/amount.
        assert_eq!(classify_by_name("is_private").score, 0.0);
        assert_eq!(classify_by_name("vat_rate").preset, None);
        assert_eq!(classify_by_name("montant_tva").preset, None);
    }

    fn table(name: &str, columns: &[(&str, &[&str])]) -> ScannedTable {
        ScannedTable {
            name: name.to_string(),
            columns: columns
                .iter()
                .map(|(column, samples)| ScannedColumn {
                    name: column.to_string(),
                    sql_type: SqlType::Text,
                    generated: false,
                    samples: samples.iter().map(|s| s.as_bytes().to_vec()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn init_groups_a_column_name_only_when_every_table_shares_the_rule() {
        let tables = vec![
            table(
                "a",
                &[("phone", &[]), ("notes", &[]), ("value", &["x@y.fr"])],
            ),
            table(
                "b",
                &[("phone", &[]), ("notes", &[]), ("value", &["x@y.fr"])],
            ),
            table(
                "c",
                &[("phone", &[]), ("notes", &[]), ("value", &["x@y.fr"])],
            ),
            // `value` here is not an email: a `*.value` pattern would wrongly cover it.
            table("d", &[("value", &["42"])]),
        ];
        let init = render_init_yaml(&tables);
        let yaml = &init.yaml;
        assert!(
            yaml.contains("  \"*.phone\": phone  # name (3 tables)"),
            "{yaml}"
        );
        // Ambiguous: grouped too, but in `review`, after the rules.
        let review = &yaml[yaml.find("review:\n").expect(yaml)..];
        assert!(
            review.contains("  \"*.notes\": null  # free text (3 tables)"),
            "{yaml}"
        );
        assert!(!yaml.contains("*.value"), "{yaml}");
        assert!(yaml.contains("    value: email"), "{yaml}");
        assert_eq!(init.to_review, 1);
        Config::parse(yaml).unwrap();
    }

    #[test]
    fn name_detection_matches_whole_words() {
        for (column, preset) in [
            ("userEmail", "email"),
            ("customer_email", "email"),
            ("emailAddress", "email"),
            ("firstname", "first_name"),
            ("contactFirstName", "first_name"),
            ("nom", "last_name"),
            ("nom_client", "last_name"),
            ("tel", "phone"),
            ("tel_portable", "phone"),
            ("phoneNumber", "phone"),
            ("zip_code", "postcode"),
            ("postalCode", "postcode"),
            ("line1", "address_line"),
            ("addressLine1", "address_line"),
            ("birthDate", "date_shift"),
            ("date_of_birth", "date_shift"),
            ("num_secu", "null"),
            ("api_token", "null"),
            ("remember_token", "null"),
            ("password_hash", "null"),
            ("client_ip", "ip"),
        ] {
            let c = classify_by_name(column);
            assert_eq!(c.preset, Some(preset), "{column}");
            assert!(c.is_mandatory(), "{column}");
        }
        assert!(classify_by_name("nickname").is_review());
        assert!(classify_by_name("comments").is_review());
    }

    #[test]
    fn name_detection_ignores_substrings_and_metadata() {
        for column in [
            "hotel",
            "nombre",
            "economy",
            "is_true",
            "security_level",
            "mailing_status",
            "cubic_meters",
            "is_private",
            "secretary",
            "content_type",
            "table_name",
            "filename",
            "address_id",
            "email_verified",
            "email_verified_at",
            "message_count",
        ] {
            assert_eq!(classify_by_name(column).score, 0.0, "{column}");
        }
    }

    #[test]
    fn content_matchers() {
        assert!(is_email("jean.dupont@gmail.com"));
        assert!(!is_email("not an email"));
        assert!(is_ip("82.64.12.201"));
        assert!(is_ip("2a01:cb00:1234::1"));
        assert!(!is_ip("1:10"));
        assert!(!is_ip("12:30:00"));
        assert!(is_phone("0612345678"));
        assert!(is_phone("+33 7 98 76 54 32"));
        assert!(is_iban("FR7630006000011234567890189"));
        assert!(!is_iban("FR7630006000011234567890188"));
        assert!(is_luhn_number("732 829 320", 9));
        assert!(!is_luhn_number("732829321", 9));
        assert!(is_luhn_number("73282932000074", 14));
        assert!(presets::vat_fr_is_valid("FR44732829320"));
        assert!(!presets::vat_fr_is_valid("FR45732829320"));
    }
}
