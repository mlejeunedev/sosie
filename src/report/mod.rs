//! End-of-run report for `transform`: counters only, never data values.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use serde::Serialize;

/// Path of the detailed JSON report, relative to the current directory.
pub const JSON_DIR: &str = ".sosie";
pub const JSON_FILE: &str = "last-report.json";

#[derive(Debug, Default, Serialize, Clone)]
pub struct ColumnReport {
    pub transformed: u64,
    pub null: u64,
    pub kept: u64,
    pub truncated: u64,
}

impl ColumnReport {
    /// A column is "touched" if it was transformed or truncated at least once.
    fn touched(&self) -> bool {
        self.transformed > 0 || self.truncated > 0
    }
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct TableReport {
    pub rows_in: u64,
    pub rows_out: u64,
    pub skipped: bool,
    pub columns: BTreeMap<String, ColumnReport>,
}

impl TableReport {
    fn touched_columns(&self) -> usize {
        self.columns.values().filter(|c| c.touched()).count()
    }

    fn truncated(&self) -> u64 {
        self.columns.values().map(|c| c.truncated).sum()
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub mode: String,
    pub tables: BTreeMap<String, TableReport>,
    pub raw_notable: Vec<String>,
    pub duration_ms: u128,
    #[serde(skip)]
    started: Option<Instant>,
}

impl Report {
    pub fn new(mode: &str) -> Self {
        Report {
            mode: mode.to_string(),
            tables: BTreeMap::new(),
            raw_notable: Vec::new(),
            duration_ms: 0,
            started: Some(Instant::now()),
        }
    }

    pub fn table_mut(&mut self, name: &str) -> &mut TableReport {
        self.tables.entry(name.to_string()).or_default()
    }

    pub fn finish(&mut self) {
        if let Some(start) = self.started.take() {
            self.duration_ms = start.elapsed().as_millis();
        }
    }

    /// Total number of rows emitted, across all tables.
    pub fn rows_out(&self) -> u64 {
        self.tables.values().map(|t| t.rows_out).sum()
    }

    /// Summary line, used when the progress bar is hidden (non-interactive
    /// stderr) and cannot print it itself:
    /// `✔ anonymize  531 840 lignes · 9 tables · 2.2s`.
    pub fn write_header<W: Write>(&self, w: &mut W) -> io::Result<()> {
        writeln!(
            w,
            "✔ {}  {} lignes · {} tables · {:.1}s",
            self.mode,
            group_thousands(self.rows_out()),
            self.tables.len(),
            self.duration_ms as f64 / 1000.0
        )
    }

    /// Human-readable summary body. By default, one line per touched table with
    /// only what needs attention; in `verbose` mode, per-column details.
    pub fn write_summary<W: Write>(&self, w: &mut W, verbose: bool) -> io::Result<()> {
        if verbose {
            self.write_columns(w)?;
        } else {
            self.write_tables(w)?;
        }
        if !self.raw_notable.is_empty() {
            writeln!(
                w,
                "  Objets non ré-générés (VIEW/TRIGGER/PROCEDURE) : {}",
                self.raw_notable.join(", ")
            )?;
        }
        writeln!(w, "  détail : {JSON_DIR}/{JSON_FILE}")
    }

    fn write_tables<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let name_width = self
            .tables
            .keys()
            .map(|k| k.chars().count())
            .max()
            .unwrap_or(0);
        let rows_width = self
            .tables
            .values()
            .filter(|t| !t.skipped && t.touched_columns() > 0)
            .map(|t| group_thousands(t.rows_out).len())
            .max()
            .unwrap_or(0);

        let mut untouched: Vec<&str> = Vec::new();
        for (table, t) in &self.tables {
            if t.skipped {
                writeln!(w, "  {table:<name_width$}  skippée")?;
                continue;
            }
            let cols = t.touched_columns();
            if cols == 0 {
                untouched.push(table);
                continue;
            }
            let rows = group_thousands(t.rows_out);
            let mut line = format!(
                "  {table:<name_width$}  {rows:>rows_width$} lignes · {cols} {}",
                plural(cols, "colonne")
            );
            let truncated = t.truncated();
            if truncated > 0 {
                line.push_str(&format!(" · ⚠ {} tronquées", group_thousands(truncated)));
            }
            writeln!(w, "{line}")?;
        }
        if !untouched.is_empty() {
            writeln!(
                w,
                "  {} {} sans transformation : {}",
                untouched.len(),
                plural(untouched.len(), "table"),
                untouched.join(", ")
            )?;
        }
        Ok(())
    }

    fn write_columns<W: Write>(&self, w: &mut W) -> io::Result<()> {
        for (table, t) in &self.tables {
            if t.skipped {
                writeln!(w, "  {table} — skippée (structure gardée, 0 ligne)")?;
                continue;
            }
            writeln!(w, "  {table} — {} lignes", group_thousands(t.rows_out))?;
            for (column, c) in &t.columns {
                if c.touched() {
                    writeln!(
                        w,
                        "    {column}: {} transformées, {} null, {} gardées, {} tronquées",
                        c.transformed, c.null, c.kept, c.truncated
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn write_json(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(dir.join(JSON_FILE), text)?;
        Ok(())
    }
}

fn plural(n: usize, word: &str) -> String {
    if n > 1 {
        format!("{word}s")
    } else {
        word.to_string()
    }
}

/// `1234567` -> `1 234 567`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(transformed: u64, null: u64, truncated: u64) -> ColumnReport {
        ColumnReport {
            transformed,
            null,
            kept: 0,
            truncated,
        }
    }

    fn sample_report() -> Report {
        let mut report = Report::new("anonymize");
        let user = report.table_mut("user");
        user.rows_out = 300_000;
        user.columns.insert("email".into(), col(300_000, 0, 0));
        user.columns.insert("phone".into(), col(299_000, 1_000, 0));
        user.columns.insert("nickname".into(), col(0, 0, 0)); // keep
        let products = report.table_mut("products");
        products.rows_out = 6_600;
        products
            .columns
            .insert("productScale".into(), col(6_600, 0, 6_600));
        report.table_mut("audit_log").skipped = true;
        report.table_mut("orders").rows_out = 19_560;
        report.table_mut("payments").rows_out = 16_380;
        report
    }

    fn render(report: &Report, verbose: bool) -> String {
        let mut buf = Vec::new();
        report.write_summary(&mut buf, verbose).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn json_report_never_contains_arbitrary_prose_field_for_values() {
        let mut report = Report::new("anonymize");
        report
            .table_mut("user")
            .columns
            .insert("email".to_string(), col(3, 0, 1));
        let json = serde_json::to_string(&report).unwrap();
        // The report must only contain counters: no "value"/"sample" field.
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"sample\""));
    }

    #[test]
    fn compact_summary_one_line_per_touched_table() {
        let out = render(&sample_report(), false);
        let expected = concat!(
            "  audit_log  skippée\n",
            "  products     6 600 lignes · 1 colonne · ⚠ 6 600 tronquées\n",
            "  user       300 000 lignes · 2 colonnes\n",
            "  2 tables sans transformation : orders, payments\n",
            "  détail : .sosie/last-report.json\n",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn compact_summary_omits_empty_sections() {
        let mut report = Report::new("anonymize");
        let user = report.table_mut("user");
        user.rows_out = 5;
        user.columns.insert("email".into(), col(5, 0, 0));
        let out = render(&report, false);
        assert!(!out.contains("sans transformation"));
        assert!(!out.contains('⚠'));
        assert!(!out.contains("skippée"));
    }

    #[test]
    fn verbose_summary_lists_columns() {
        let out = render(&sample_report(), true);
        assert!(out.contains("  user — 300 000 lignes\n"));
        assert!(out.contains("    email: 300000 transformées, 0 null, 0 gardées, 0 tronquées\n"));
        assert!(
            out.contains("    phone: 299000 transformées, 1000 null, 0 gardées, 0 tronquées\n")
        );
        assert!(!out.contains("nickname"));
        assert!(out.contains("  audit_log — skippée (structure gardée, 0 ligne)\n"));
    }

    #[test]
    fn header_line() {
        let mut report = sample_report();
        report.duration_ms = 2_240;
        let mut buf = Vec::new();
        report.write_header(&mut buf).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "✔ anonymize  342 540 lignes · 5 tables · 2.2s\n"
        );
    }

    #[test]
    fn groups_thousands_with_spaces() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1 000");
        assert_eq!(group_thousands(1_234_567), "1 234 567");
    }
}
