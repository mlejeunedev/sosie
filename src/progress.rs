//! Barre de progression sur stderr pour les commandes qui lisent un dump.
//!
//! La barre se base sur les octets consommés du flux d'entrée : quand la
//! taille totale est connue (fichier), on affiche pourcentage, débit et ETA ;
//! sinon (stdin) un spinner avec les octets lus et le débit. Elle est
//! automatiquement masquée si stderr n'est pas un terminal (CI, redirection).

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
    /// `total_bytes` : taille du flux d'entrée si elle est connue.
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

    /// Enveloppe le flux d'entrée pour faire avancer la barre à chaque lecture.
    pub fn wrap_read<R: Read>(&self, reader: R) -> ProgressBarIter<R> {
        self.bar.wrap_read(reader)
    }

    /// Met à jour la table courante et le compteur de lignes.
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

    /// Remplace la barre par une ligne de bilan :
    /// `✔ analyse  6 tables · 42.83 MiB en 0.4s`.
    ///
    /// Une analyse rapide peut se terminer avant le premier rafraîchissement
    /// de la barre ; cette ligne garantit un retour visible dans tous les cas.
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
        // indicatif laisse le curseur en fin de ligne : sans ce saut, la
        // prochaine écriture sur stdout viendrait se coller au bilan.
        if !self.bar.is_hidden() {
            eprintln!();
        }
    }

    /// Bilan de `transform` : `✔ anonymize  531 840 lignes · 9 tables · 42.83 MiB en 2.2s`.
    pub fn finish_transform(&self, rows: u64, tables: usize) {
        self.finish(&format!(
            "{} lignes · {tables} tables",
            group_thousands(rows)
        ));
    }

    /// `true` si la barre n'est pas affichée (stderr n'est pas un terminal) :
    /// le bilan doit alors être écrit explicitement par l'appelant.
    pub fn is_hidden(&self) -> bool {
        self.bar.is_hidden()
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        // En cas d'erreur, on ne laisse pas une barre à moitié dessinée
        // au-dessus du message d'erreur.
        if !self.bar.is_finished() {
            self.bar.finish_and_clear();
        }
    }
}
