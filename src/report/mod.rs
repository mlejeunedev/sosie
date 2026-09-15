//! Rapport de fin d'exécution de `transform` : uniquement des compteurs,
//! jamais une valeur de donnée.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Default, Serialize, Clone)]
pub struct ColumnReport {
    pub transformed: u64,
    pub null: u64,
    pub kept: u64,
    pub truncated: u64,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct TableReport {
    pub rows_in: u64,
    pub rows_out: u64,
    pub skipped: bool,
    pub columns: BTreeMap<String, ColumnReport>,
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

    pub fn print_terminal(&self) {
        println!(
            "sosie transform — terminé en {:.1}s",
            self.duration_ms as f64 / 1000.0
        );
        for (table, t) in &self.tables {
            if t.skipped {
                println!("  {table} — skippée (structure gardée, 0 ligne)");
                continue;
            }
            println!("  {table} — {} lignes", t.rows_out);
            for (column, c) in &t.columns {
                if c.transformed > 0 || c.truncated > 0 {
                    println!(
                        "    {column}: {} transformées, {} null, {} gardées, {} tronquées",
                        c.transformed, c.null, c.kept, c.truncated
                    );
                }
            }
        }
        if !self.raw_notable.is_empty() {
            println!(
                "  Objets non ré-générés (VIEW/TRIGGER/PROCEDURE) : {}",
                self.raw_notable.join(", ")
            );
        }
    }

    pub fn write_json(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(dir.join("last-report.json"), text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_report_never_contains_arbitrary_prose_field_for_values() {
        let mut report = Report::new("anonymize");
        report.table_mut("user").columns.insert(
            "email".to_string(),
            ColumnReport {
                transformed: 3,
                null: 0,
                kept: 0,
                truncated: 1,
            },
        );
        let json = serde_json::to_string(&report).unwrap();
        // Le rapport ne doit contenir que des compteurs : pas de champ "value"/"sample".
        assert!(!json.contains("\"value\""));
        assert!(!json.contains("\"sample\""));
    }
}
