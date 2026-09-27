use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sosie::config::Config;
use sosie::dump::Value;
use sosie::presets::{self, ColumnCtx};
use sosie::report::Report;
use sosie::scan;
use sosie::transform;

mod progress;
use progress::Progress;

#[derive(Parser, Debug)]
#[command(
    name = "sosie",
    version,
    about = "Anonymise ou pseudonymise un dump mysqldump, en streaming."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Transform a dump according to a config, in streaming mode.
    Transform(TransformArgs),
    /// Analyze a dump and generate a starter config file.
    Init(InitArgs),
    /// Check that a config covers every sensitive column of a dump.
    Check(CheckArgs),
    /// List available presets with a generated example.
    Presets,
}

#[derive(Parser, Debug)]
struct TransformArgs {
    /// Source file (`mysqldump`). Defaults to stdin.
    #[arg(long)]
    from: Option<PathBuf>,
    /// Config file (`sosie.yaml`).
    #[arg(long)]
    config: PathBuf,
    /// Output file. Defaults to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Do everything except write the output (validates config and dump).
    #[arg(long)]
    dry_run: bool,
    /// Show per-column details in the final summary.
    #[arg(long)]
    verbose: bool,
}

#[derive(Parser, Debug)]
struct InitArgs {
    /// `mysqldump` dump to analyze.
    #[arg(long)]
    from: PathBuf,
    /// Config file to write.
    #[arg(long, default_value = "sosie.yaml")]
    out: PathBuf,
}

#[derive(Parser, Debug)]
struct CheckArgs {
    /// `mysqldump` dump to analyze.
    #[arg(long)]
    from: PathBuf,
    /// `sosie.yaml` config file to check.
    #[arg(long)]
    config: PathBuf,
}

fn main() -> ExitCode {
    let args = Cli::parse();

    let result = match args.command {
        Command::Transform(a) => cmd_transform(a),
        Command::Init(a) => cmd_init(a),
        Command::Check(a) => cmd_check(a),
        Command::Presets => cmd_presets(),
    };

    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("erreur : {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_transform(args: TransformArgs) -> Result<bool> {
    let config = Config::load(&args.config)?;
    let mode_label = match config.mode {
        sosie::config::Mode::Anonymize => "anonymize",
        sosie::config::Mode::Pseudonymize => "pseudonymize",
    };
    let mut report = Report::new(mode_label);

    let (reader, total_bytes) = open_input(args.from.as_deref())?;
    let mut progress = Progress::new(total_bytes, mode_label);
    let reader = progress.wrap_read(reader);
    let on_progress = |ev: transform::Progress<'_>| progress.update(ev);

    if args.dry_run {
        transform::run_with_progress(&config, reader, io::sink(), &mut report, on_progress)?;
    } else {
        match &args.out {
            Some(path) => {
                let tmp_path = tmp_path_for(path);
                let file = File::create(&tmp_path)
                    .with_context(|| format!("création de {}", tmp_path.display()))?;
                let result = transform::run_with_progress(
                    &config,
                    reader,
                    BufWriter::new(file),
                    &mut report,
                    on_progress,
                );
                match result {
                    Ok(()) => {
                        std::fs::rename(&tmp_path, path).with_context(|| {
                            format!(
                                "renommage de {} vers {}",
                                tmp_path.display(),
                                path.display()
                            )
                        })?;
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp_path);
                        return Err(e);
                    }
                }
            }
            None => {
                let stdout = io::stdout();
                transform::run_with_progress(
                    &config,
                    reader,
                    BufWriter::new(stdout.lock()),
                    &mut report,
                    on_progress,
                )?;
            }
        }
    }

    report.finish();
    // The progress bar prints its own summary line; when it is hidden
    // (non-interactive stderr), print it here.
    let bar_hidden = progress.is_hidden();
    progress.finish_transform(report.rows_out(), report.tables.len());

    // Keep the report off stdout when the dump is written there.
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut w: Box<dyn Write> = if args.out.is_none() && !args.dry_run {
        Box::new(stderr.lock())
    } else {
        Box::new(stdout.lock())
    };
    if bar_hidden {
        let _ = report.write_header(&mut w);
    }
    let _ = report.write_summary(&mut w, args.verbose);
    let _ = report.write_json(std::path::Path::new(sosie::report::JSON_DIR));
    Ok(true)
}

/// Opens the input stream (`path`, or stdin) and returns its size when known,
/// to scale the progress bar.
fn open_input(path: Option<&std::path::Path>) -> Result<(Box<dyn Read>, Option<u64>)> {
    match path {
        Some(path) => {
            let file =
                File::open(path).with_context(|| format!("ouverture de {}", path.display()))?;
            let len = file.metadata().ok().map(|m| m.len()).filter(|&l| l > 0);
            Ok((Box::new(file), len))
        }
        None => Ok((Box::new(io::stdin()), None)),
    }
}

/// Analyzes a dump with an `analyse` spinner on stderr.
fn scan_with_progress(path: &std::path::Path) -> Result<Vec<scan::ScannedTable>> {
    let (reader, total_bytes) = open_input(Some(path))?;
    let progress = Progress::new(total_bytes, "analyse");
    let tables = scan::scan_dump(progress.wrap_read(reader))?;
    progress.finish(&format!("{} tables", tables.len()));
    Ok(tables)
}

fn tmp_path_for(path: &std::path::Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".sosie-tmp");
    path.with_file_name(name)
}

fn cmd_init(args: InitArgs) -> Result<bool> {
    let tables = scan_with_progress(&args.from)?;
    let yaml = scan::render_init_yaml(&tables);
    std::fs::write(&args.out, yaml)
        .with_context(|| format!("écriture de {}", args.out.display()))?;
    println!(
        "{} généré à partir de {}",
        args.out.display(),
        args.from.display()
    );
    println!("Relis-le et ajuste les presets avant de lancer `sosie check`.");
    Ok(true)
}

fn cmd_check(args: CheckArgs) -> Result<bool> {
    let config = Config::load(&args.config)?;
    let tables = scan_with_progress(&args.from)?;
    let classifications = scan::classify(&tables);

    let exempt = |table: &str| {
        config.skip_tables.iter().any(|t| t == table)
            || config.truncate_tables.iter().any(|t| t == table)
    };
    let has_rule = |table: &str, column: &str| {
        config
            .tables
            .get(table)
            .map(|cols| cols.contains_key(column))
            .unwrap_or(false)
    };
    let missing = scan::missing_mandatory(&classifications, has_rule, exempt);
    let review = scan::needs_review(&classifications, has_rule, exempt);

    if missing.is_empty() && review.is_empty() {
        println!(
            "check OK : {} tables, aucune colonne sensible sans règle.",
            tables.len()
        );
        return Ok(true);
    }

    if !missing.is_empty() {
        println!("Colonnes sensibles sans règle (score >= 0.8) :");
        for (table, column, c) in &missing {
            println!("  {table}.{column}  — {}", c.reason);
        }
    }
    if !review.is_empty() {
        println!("Colonnes à trancher manuellement (score 0.4-0.8) :");
        for (table, column, c) in &review {
            println!("  {table}.{column}  — {}", c.reason);
        }
    }
    println!(
        "\ncheck ÉCHOUÉ. Ajoute une règle explicite (ou `keep`) pour chaque colonne ci-dessus."
    );
    Ok(false)
}

fn cmd_presets() -> Result<bool> {
    println!("{:<14} exemple", "preset");
    for name in presets::KNOWN_PRESETS {
        let sample = sample_value_for(name);
        let preset = presets::build(name, &Default::default())?;
        let seed = presets::seed_for(b"sosie-presets-demo", name, sample.as_bytes());
        let out = preset.apply(
            &Value::Str(std::borrow::Cow::Borrowed(sample.as_bytes())),
            &seed,
            &ColumnCtx { max_len: None },
        );
        let rendered = match out {
            Value::Str(s) => String::from_utf8_lossy(&s).into_owned(),
            Value::Null => "NULL".to_string(),
            Value::Raw(b) => String::from_utf8_lossy(b).into_owned(),
        };
        println!("{name:<14} {sample} -> {rendered}");
    }
    Ok(true)
}

fn sample_value_for(preset: &str) -> String {
    match preset {
        "phone" => "0612345678".to_string(),
        "date_shift" => "2000-01-01".to_string(),
        "iban" => "FR7630006000011234567890189".to_string(),
        "bic" => "AGRIFRPPXXX".to_string(),
        "ip" => "192.168.1.1".to_string(),
        "postcode" => "75001".to_string(),
        _ => "exemple".to_string(),
    }
}
