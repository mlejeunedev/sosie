use std::fs::File;
use std::io::{self, BufWriter, Read};
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
    /// Transforme un dump selon une config, en streaming.
    Transform(TransformArgs),
    /// Analyse un dump et propose un fichier de config de départ.
    Init(InitArgs),
    /// Vérifie qu'une config couvre bien toutes les colonnes sensibles d'un dump.
    Check(CheckArgs),
    /// Liste les presets disponibles avec un exemple généré.
    Presets,
}

#[derive(Parser, Debug)]
struct TransformArgs {
    /// Fichier source (`mysqldump`). Par défaut : stdin.
    #[arg(long)]
    from: Option<PathBuf>,
    /// Fichier de config `sosie.yaml`.
    #[arg(long)]
    config: PathBuf,
    /// Fichier de sortie. Par défaut : stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Fait tout sauf écrire la sortie (valide la config et le dump).
    #[arg(long)]
    dry_run: bool,
}

#[derive(Parser, Debug)]
struct InitArgs {
    /// Dump `mysqldump` à analyser.
    #[arg(long)]
    from: PathBuf,
    /// Fichier de config à écrire.
    #[arg(long, default_value = "sosie.yaml")]
    out: PathBuf,
}

#[derive(Parser, Debug)]
struct CheckArgs {
    /// Dump `mysqldump` à analyser.
    #[arg(long)]
    from: PathBuf,
    /// Fichier de config `sosie.yaml` à vérifier.
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

    let reader: Box<dyn Read> = match &args.from {
        Some(path) => {
            Box::new(File::open(path).with_context(|| format!("ouverture de {}", path.display()))?)
        }
        None => Box::new(io::stdin()),
    };

    if args.dry_run {
        transform::run(&config, reader, io::sink(), &mut report)?;
    } else {
        match &args.out {
            Some(path) => {
                let tmp_path = tmp_path_for(path);
                let file = File::create(&tmp_path)
                    .with_context(|| format!("création de {}", tmp_path.display()))?;
                let result = transform::run(&config, reader, BufWriter::new(file), &mut report);
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
                transform::run(&config, reader, BufWriter::new(stdout.lock()), &mut report)?;
            }
        }
    }

    report.finish();
    report.print_terminal();
    let _ = report.write_json(std::path::Path::new(".sosie"));
    Ok(true)
}

fn tmp_path_for(path: &std::path::Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".sosie-tmp");
    path.with_file_name(name)
}

fn cmd_init(args: InitArgs) -> Result<bool> {
    let file =
        File::open(&args.from).with_context(|| format!("ouverture de {}", args.from.display()))?;
    let tables = scan::scan_dump(file)?;
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
    let file =
        File::open(&args.from).with_context(|| format!("ouverture de {}", args.from.display()))?;
    let tables = scan::scan_dump(file)?;
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
