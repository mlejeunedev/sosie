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

const EXCLUDED_GENERIC_NAME: &[&str] = &["table_name", "file_name", "class_name", "role_name"];

/// Signal 1: detection by column name. See `docs/CAHIER_DES_CHARGES.md` §5.5.
pub fn classify_by_name(column: &str) -> Classification {
    let n = normalize(column);
    let hit = |patterns: &[&str]| patterns.iter().any(|p| n.contains(p));

    if hit(&["email", "mail", "courriel"]) {
        return Classification {
            score: 0.9,
            preset: Some("email"),
            reason: "name".into(),
        };
    }
    if hit(&["first_name", "firstname", "prenom", "given_name"]) {
        return Classification {
            score: 0.9,
            preset: Some("first_name"),
            reason: "name".into(),
        };
    }
    if hit(&["last_name", "lastname", "surname", "family_name"])
        || n.contains("nom") && !n.contains("prenom")
    {
        return Classification {
            score: 0.9,
            preset: Some("last_name"),
            reason: "name".into(),
        };
    }
    if hit(&["phone", "tel", "mobile", "telephone"]) {
        return Classification {
            score: 0.9,
            preset: Some("phone"),
            reason: "name".into(),
        };
    }
    if hit(&["iban", "bban", "account_number"]) {
        return Classification {
            score: 0.9,
            preset: Some("iban"),
            reason: "name".into(),
        };
    }
    if hit(&["bic", "swift"]) {
        return Classification {
            score: 0.9,
            preset: Some("bic"),
            reason: "name".into(),
        };
    }
    if hit(&["ip_address", "remote_addr"]) || n == "ip" || n.ends_with("_ip") {
        return Classification {
            score: 0.9,
            preset: Some("ip"),
            reason: "name".into(),
        };
    }
    if hit(&["address", "adresse", "street", "rue", "line1", "line2"]) {
        return Classification {
            score: 0.9,
            preset: Some("address_line"),
            reason: "name".into(),
        };
    }
    if hit(&["city", "ville", "town"]) {
        return Classification {
            score: 0.9,
            preset: Some("city"),
            reason: "name".into(),
        };
    }
    if hit(&["zip", "postcode", "postal_code", "code_postal"]) {
        return Classification {
            score: 0.9,
            preset: Some("postcode"),
            reason: "name".into(),
        };
    }
    if hit(&["birth", "dob", "naissance"]) {
        return Classification {
            score: 0.9,
            preset: Some("date_shift"),
            reason: "name (birth)".into(),
        };
    }
    if hit(&["ssn", "nir", "secu", "social_security"]) {
        return Classification {
            score: 0.9,
            preset: Some("null"),
            reason: "name".into(),
        };
    }
    if hit(&[
        "password", "passwd", "pwd", "token", "secret", "api_key", "salt",
    ]) {
        return Classification {
            score: 0.9,
            preset: Some("null"),
            reason: "name (token)".into(),
        };
    }
    if hit(&["siret", "siren", "vat", "tva"]) {
        return Classification {
            score: 0.5,
            preset: None,
            reason: "name (company identifier, no preset in v0.1)".into(),
        };
    }
    if hit(&[
        "note",
        "comment",
        "message",
        "body",
        "content",
        "description",
    ]) {
        return Classification {
            score: 0.5,
            preset: None,
            reason: "free text".into(),
        };
    }

    // Generic "name"/"holder" signal, combinable, with exclusions.
    if !EXCLUDED_GENERIC_NAME.contains(&n.as_str()) {
        let mut score: f64 = 0.0;
        let mut hits = Vec::new();
        if n.contains("name") {
            score += 0.5;
            hits.push("name");
        }
        if n.contains("holder") {
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
    let s = s.trim();
    s.contains(':') && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
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

/// Generates a starter `sosie.yaml`: columns `>= 0.8` with their suggested
/// preset (a comment names the triggering signal), plus a `review` section
/// for the `0.4..0.8` gray zone. Written by hand rather than through serde
/// to preserve the comments.
pub fn render_init_yaml(tables: &[ScannedTable]) -> String {
    let classifications = classify(tables);
    let mut by_table: BTreeMap<&str, Vec<(&str, &Classification)>> = BTreeMap::new();
    for ((table, column), c) in &classifications {
        if c.is_mandatory() {
            by_table
                .entry(table.as_str())
                .or_default()
                .push((column.as_str(), c));
        }
    }

    let mut out = String::new();
    out.push_str("version: 1\n\n");
    out.push_str("source:\n  kind: mysql\n\n");
    out.push_str("mode: anonymize\n\n");
    out.push_str("defaults:\n  locale: fr_FR\n  on_unclassified: fail\n\n");
    out.push_str("tables:\n");
    for (table, columns) in &by_table {
        out.push_str(&format!("  {table}:\n"));
        for (column, c) in columns {
            let rule = match c.preset {
                Some("date_shift") => "{ preset: date_shift, days: 365 }".to_string(),
                Some(preset) => preset.to_string(),
                None => "review".to_string(),
            };
            out.push_str(&format!("    {column}: {rule}  # {}\n", c.reason));
        }
    }
    out.push('\n');

    out.push_str("review:\n");
    let mut review: Vec<(&str, &str, &Classification)> =
        needs_review(&classifications, |_, _| false, |_| false);
    review.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    if review.is_empty() {
        out.push_str("  []\n");
    } else {
        for (table, column, c) in review {
            out.push_str(&format!("  - {table}.{column}  # {}\n", c.reason));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn content_matchers() {
        assert!(is_email("jean.dupont@gmail.com"));
        assert!(!is_email("not an email"));
        assert!(is_ip("82.64.12.201"));
        assert!(is_ip("2a01:cb00:1234::1"));
        assert!(is_phone("0612345678"));
        assert!(is_phone("+33 7 98 76 54 32"));
        assert!(is_iban("FR7630006000011234567890189"));
        assert!(!is_iban("FR7630006000011234567890188"));
    }
}
