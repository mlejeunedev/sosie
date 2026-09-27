//! Progress bar on stderr for commands that read a dump.
//!
//! Progress is based on bytes consumed from the input stream: with a known
//! total size (file), it shows percentage, throughput and ETA; otherwise
//! (stdin), a spinner with bytes read and throughput. Hidden automatically
//! when stderr is not a terminal (CI, redirection).

use std::io::Read;
use std::time::Duration;

use indicatif::{HumanBytes, ProgressBar, ProgressBarIter, ProgressDrawTarget, ProgressStyle};

use sosie::report::group_thousands;
use sosie::transform;

const TICK_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";

pub struct Progress {
    bar: ProgressBar,
    table: Option<String>,
    rows: u64,
}

impl Progress {
    /// `total_bytes`: input stream size, if known.
    pub fn new(total_bytes: Option<u64>, label: &str) -> Self {
        let bar = match total_bytes {
            Some(len) => {
                let bar = ProgressBar::with_draw_target(Some(len), ProgressDrawTarget::stderr());
                bar.set_style(
                    ProgressStyle::with_template(
                        "{spinner:.cyan} {prefix:.bold} {bar:28.cyan/240} {percent:>3}% \
                         {bytes:>9}/{total_bytes:<9} {bytes_per_sec:>10} · ETA {eta:<4} {msg:.dim}",
                    )
                    .expect("template valide")
                    .tick_chars(TICK_CHARS)
                    .progress_chars("━╸─"),
                );
                bar
            }
            None => {
                let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
                bar.set_style(
                    ProgressStyle::with_template(
                        "{spinner:.cyan} {prefix:.bold} {bytes:>9} lus {bytes_per_sec:>10} · {elapsed} {msg:.dim}",
                    )
                    .expect("template valide")
                    .tick_chars(TICK_CHARS),
                );
                bar
            }
        };
        bar.set_prefix(label.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));
        Self {
            bar,
            table: None,
            rows: 0,
        }
    }

    /// Wraps the input stream so that each read advances the bar.
    pub fn wrap_read<R: Read>(&self, reader: R) -> ProgressBarIter<R> {
        self.bar.wrap_read(reader)
    }

    /// Updates the current table and the row counter.
    pub fn update(&mut self, ev: transform::Progress<'_>) {
        match ev {
            transform::Progress::Table(name) => self.table = Some(name.to_string()),
            transform::Progress::Rows(n) => self.rows = n,
        }
        let mut msg = format!("{} lignes", group_thousands(self.rows));
        if let Some(table) = &self.table {
            msg.push_str(" · ");
            msg.push_str(table);
        }
        self.bar.set_message(msg);
    }

    /// Replaces the bar with a summary line:
    /// `✔ analyse  6 tables · 42.83 MiB en 0.4s`.
    ///
    /// A fast scan may finish before the first redraw; this line guarantees
    /// visible feedback in every case.
    pub fn finish(&self, detail: &str) {
        let elapsed = self.bar.elapsed();
        let duration = if elapsed.as_secs() < 1 {
            format!("{} ms", elapsed.as_millis())
        } else {
            format!("{:.1}s", elapsed.as_secs_f64())
        };
        let mut summary = String::new();
        if !detail.is_empty() {
            summary.push_str(detail);
            summary.push_str(" · ");
        }
        summary.push_str(&format!(
            "{} en {duration}",
            HumanBytes(self.bar.position())
        ));
        self.bar.set_style(
            ProgressStyle::with_template("{spinner:.green} {prefix:.bold}  {msg}")
                .expect("template valide")
                .tick_chars("✔✔"),
        );
        self.bar.finish_with_message(summary);
        // indicatif leaves the cursor at the end of the line; without this newline,
        // the next write to stdout would be glued to the summary.
        if !self.bar.is_hidden() {
            eprintln!();
        }
    }

    /// `transform` summary: `✔ anonymize  531 840 lignes · 9 tables · 42.83 MiB en 2.2s`.
    pub fn finish_transform(&self, rows: u64, tables: usize) {
        self.finish(&format!(
            "{} lignes · {tables} tables",
            group_thousands(rows)
        ));
    }

    /// `true` if the bar is hidden (stderr is not a terminal): the caller must
    /// then print the summary itself.
    pub fn is_hidden(&self) -> bool {
        self.bar.is_hidden()
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        // On error, don't leave a half-drawn bar above the error message.
        if !self.bar.is_finished() {
            self.bar.finish_and_clear();
        }
    }
}
