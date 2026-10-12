//! End-to-end tests: run the real `sosie` binary on the example fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BASIC_DUMP: &str = "fixtures/exemples/dumps/01_basic.sql";
const PSEUDO_KEY: &str = "test-key-do-not-use-in-prod";

/// Real personal values present in `01_basic.sql` that must never survive a transform.
const ORIGINAL_PII: &[&str] = &[
    "jean.dupont@gmail.com",
    "lucas.martin@orange.fr",
    "connor@example.com",
];

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Fresh scratch directory per test; `transform` writes `.sosie/` into its cwd.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sosie-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sosie(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sosie"))
        .current_dir(cwd)
        .env_remove("SOSIE_KEY")
        .args(args)
        .output()
        .expect("failed to run sosie")
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn check_passes_with_complete_config() {
    let dir = scratch("check-ok");
    let out = sosie(
        &dir,
        &[
            "check",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--config",
            path_str(&fixture("fixtures/exemples/configs/01_basic.yaml")),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn check_fails_on_uncovered_sensitive_columns() {
    let dir = scratch("check-ko");
    let out = sosie(
        &dir,
        &[
            "check",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--config",
            path_str(&fixture("fixtures/exemples/configs/02_incomplete.yaml")),
        ],
    );
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("bank_account.iban"), "{text}");
}

#[test]
fn init_generates_config_for_sensitive_columns() {
    let dir = scratch("init");
    let generated = dir.join("sosie.yaml");
    let out = sosie(
        &dir,
        &[
            "init",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--out",
            path_str(&generated),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let yaml = fs::read_to_string(&generated).unwrap();
    for expected in [
        "email: email",
        "first_name: first_name",
        "iban: iban",
        "ip_address: ip",
        "exclude: []",
        "  order.notes: null  # free text",
        "  audit_log.payload: null  # JSON",
    ] {
        assert!(yaml.contains(expected), "missing `{expected}` in:\n{yaml}");
    }
    // An ambiguous column lives in `review:` only, never also in `tables:`.
    assert!(!yaml.contains("    notes:"), "{yaml}");
}

#[test]
fn review_entries_must_be_moved_to_columns_or_exclude() {
    let dir = scratch("init-review");
    let generated = dir.join("sosie.yaml");
    let dump = fixture(BASIC_DUMP);
    let out = sosie(
        &dir,
        &[
            "init",
            "--from",
            path_str(&dump),
            "--out",
            path_str(&generated),
        ],
    );
    assert!(out.status.success());
    let check = |dir: &Path| {
        sosie(
            dir,
            &[
                "check",
                "--from",
                path_str(&dump),
                "--config",
                path_str(&generated),
            ],
        )
    };

    // Decisions pending: check fails and lists them.
    let out = check(&dir);
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("left in `review:`"), "{text}");
    assert!(text.contains("order.notes"), "{text}");

    let yaml = fs::read_to_string(&generated).unwrap();
    let head = &yaml[..yaml.find("review:\n").unwrap()];

    // Emptying `review:` without deciding drops the default rules: still failing.
    fs::write(&generated, format!("{head}review: {{}}\n")).unwrap();
    assert!(!check(&dir).status.success());

    // Each entry moved to `columns` (rule confirmed) or `exclude` (kept as is).
    let decided = head
        .replace(
            "columns: {}",
            "columns:\n  order.notes: null\n  audit_log.payload: null\n  user.nickname: null",
        )
        .replace("exclude: []", "exclude:\n  - product.name");
    fs::write(&generated, format!("{decided}review: {{}}\n")).unwrap();
    let out = check(&dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // A column in `exclude` must not also have a rule.
    let conflicting = decided.replace("  - product.name", "  - product.name\n  - \"*.notes\"");
    fs::write(&generated, format!("{conflicting}review: {{}}\n")).unwrap();
    let out = check(&dir);
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("order.notes  — in tables/columns and exclude"),
        "{text}"
    );
}

#[test]
fn columns_patterns_cover_columns_and_exact_rules_win() {
    let dir = scratch("patterns");
    let config = dir.join("sosie.yaml");
    // `*.email` covers user.email and nothing else; order.customer_email keeps
    // its exact rule. `*.notes` is overridden by an exact `keep`.
    fs::write(
        &config,
        r#"version: 1
source: { kind: mysql }
mode: anonymize
tables:
  user:
    first_name: first_name
    last_name: last_name
    phone: phone
    birth_date: { preset: date_shift, days: 365 }
    password: null
    api_token: null
    nickname: null
  address: { line1: address_line, line2: null, city: city, postcode: postcode }
  order:
    customer_email: email
    notes: keep
  bank_account: { iban: iban, bic: bic, holder_name: full_name }
  product: { name: keep }
skip_tables: [audit_log]
columns:
  "*.email": email
  "*.notes": null
  "nope_*.*": keep
"#,
    )
    .unwrap();
    let dump = fixture(BASIC_DUMP);
    let out = sosie(
        &dir,
        &[
            "check",
            "--from",
            path_str(&dump),
            "--config",
            path_str(&config),
        ],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("`nope_*.*` matches no column"), "{text}");

    let out = sosie(
        &dir,
        &[
            "transform",
            "--from",
            path_str(&dump),
            "--config",
            path_str(&config),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    for pii in ORIGINAL_PII {
        assert!(!sql.contains(pii), "`{pii}` survived the transform");
    }
    // `notes: keep` (exact) won over `*.notes: null`: the original text survives.
    assert!(
        sql.contains("Deliver before 6pm"),
        "exact `keep` should win over the pattern"
    );
}

#[test]
fn transform_anonymize_removes_original_pii() {
    let dir = scratch("anonymize");
    let clean = dir.join("clean.sql");
    let out = sosie(
        &dir,
        &[
            "transform",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--config",
            path_str(&fixture("fixtures/exemples/configs/01_basic.yaml")),
            "--out",
            path_str(&clean),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let sql = fs::read_to_string(&clean).unwrap();
    assert!(sql.contains("CREATE TABLE `user`"));
    assert!(sql.contains("INSERT INTO `user`"));
    for pii in ORIGINAL_PII {
        assert!(!sql.contains(pii), "`{pii}` leaked into the output");
    }
}

#[test]
fn transform_pseudonymize_is_stable_across_runs() {
    let dir = scratch("pseudonymize");
    let run = |name: &str| {
        let target = dir.join(name);
        let out = Command::new(env!("CARGO_BIN_EXE_sosie"))
            .current_dir(&dir)
            .env("SOSIE_KEY", PSEUDO_KEY)
            .args([
                "transform",
                "--from",
                path_str(&fixture(BASIC_DUMP)),
                "--config",
                path_str(&fixture("fixtures/exemples/configs/03_pseudonymize.yaml")),
                "--out",
                path_str(&target),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        fs::read(&target).unwrap()
    };

    let first = run("a.sql");
    assert_eq!(
        first,
        run("b.sql"),
        "pseudonymized output must be byte-identical"
    );
    let text = String::from_utf8_lossy(&first);
    for pii in ORIGINAL_PII {
        assert!(!text.contains(pii), "`{pii}` leaked into the output");
    }
}

#[test]
fn transform_pseudonymize_requires_key() {
    let dir = scratch("pseudonymize-nokey");
    let out = sosie(
        &dir,
        &[
            "transform",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--config",
            path_str(&fixture("fixtures/exemples/configs/03_pseudonymize.yaml")),
            "--dry-run",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("SOSIE_KEY"));
}

#[cfg(unix)]
#[test]
fn transform_can_write_to_dev_null() {
    let dir = scratch("dev-null");
    let out = sosie(
        &dir,
        &[
            "transform",
            "--from",
            path_str(&fixture(BASIC_DUMP)),
            "--config",
            path_str(&fixture("fixtures/exemples/configs/01_basic.yaml")),
            "--out",
            "/dev/null",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
