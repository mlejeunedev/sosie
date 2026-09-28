# Sosie

[![crates.io](https://img.shields.io/crates/v/sosie.svg)](https://crates.io/crates/sosie)
[![pipeline](https://gitlab.com/mlejeune/sosie/badges/main/pipeline.svg)](https://gitlab.com/mlejeune/sosie/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE.md)

**Copy a production MySQL database to a developer laptop without a single real piece of personal data ending up there.**

Sosie reads a `mysqldump` dump, replaces sensitive columns (email, name, phone, IBAN, address...) with fake but plausible values, and writes back a valid SQL dump — streaming, without ever loading the whole file into memory.

```bash
mysqldump my_db | sosie transform --config sosie.yaml > dump_clean.sql
mysql my_db_dev < dump_clean.sql
```

```
-- before
(1,'jean.dupont@gmail.com','Jean','Dupont','0612345678','1985-03-14','$2y$13$abcd…','tok_a1b2c3d4e5f6','jeanjean',…)
-- after
(1,'claire.andre.504a71@example.org','Theo','Lefebvre','0795958940','1985-08-26','$2y$13$DEVONLY…',NULL,NULL,…)
```

## Why

A raw production dump on a developer laptop is a personal data leak waiting to happen. The usual alternatives are poor: fixtures never look like real production, a homemade SQL script is slow and quickly goes stale, and re-running the same anonymizing queries by hand every time doesn't scale.

Sosie automates this with one simple rule: **by default, it refuses to run if a column that looks like personal data has no explicit rule.**

## Features

- **Streaming** — the dump is processed one statement at a time: 1 GB and 17 million rows in ~30 s, under 100 MB of RAM (Apple M1 Pro). Only the deduplication state of `UNIQUE` columns grows with the data.
- **Guided setup** — `sosie init` scans the schema *and the content* of your dump and generates a ready-to-review config. You only decide the ambiguous cases.
- **Safe by default** — `check` fails in CI if a sensitive column has no rule; `transform` refuses to start as a last resort.
- **Fakes that look real** — IBANs with a valid checksum, well-formed phone numbers, plausible addresses and dates. `UNIQUE` / `PRIMARY KEY` values stay unique, `NOT NULL` columns never get a `NULL`.
- **Anonymize or pseudonymize** — a random key per run (irreversible), or a stable key from `SOSIE_KEY` for reproducible output across exports.
- **Byte-exact passthrough** — everything that isn't transformed is written back unchanged.
- **Data-free reports** — the end-of-run summary only contains counters, never a data value.

## Installation

**With Rust installed** (a recent stable toolchain from [rustup](https://rustup.rs); works on any OS, servers included):

```bash
cargo install --locked sosie
```

**Without Rust:** download the prebuilt binary for your platform (Linux, macOS Intel / Apple Silicon, Windows) from the [GitHub Releases page](https://github.com/mlejeunedev/sosie/releases), extract it and run `./sosie --help`.

- **macOS:** the binary isn't signed, so macOS blocks it on first launch ("Apple could not verify…"). Remove the quarantine flag once:
  ```bash
  xattr -d com.apple.quarantine ./sosie
  ```
- **Linux:** the prebuilt binary needs a recent glibc (Ubuntu 24.04+, Debian 13+ or equivalent). On older distributions, use `cargo install` above.

**From source:**

```bash
git clone https://github.com/mlejeunedev/sosie.git
cd sosie
cargo build --release
./target/release/sosie --help
```

## Quick start

**1. Dump the database to anonymize**

```bash
mysqldump --single-transaction my_db > dump.sql
```

**2. Generate a starter config** — Sosie analyzes the schema and a sample of the data, and proposes a `sosie.yaml`:

```bash
sosie init --from dump.sql --out sosie.yaml
```

**3. Review and complete the config.** `init` automatically covers what it recognizes with confidence (email, phone, IBAN...), and leaves a `review:` section for ambiguous cases (`notes`, `nickname`, a JSON column containing a sensitive key...) — the only moment a human has to decide:

```yaml
tables:
  order:
    notes: null       # free text that sometimes contained a phone number -> empty it
  product:
    name: keep        # product name, not a person -> false positive, keep it
```

**4. Check that everything is covered**

```bash
sosie check --from dump.sql --config sosie.yaml
```

**5. Transform**

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

`dump_clean.sql` is a valid SQL dump, importable as is, where personal data has been replaced by consistent fake values (same structure, same format, `NOT NULL` constraints respected).

**6. Import locally**

```bash
mysql my_db_dev < dump_clean.sql
```

## Documentation

- [`docs/USAGE.md`](docs/USAGE.md) — full reference: commands, a complete walkthrough, presets, limitations.
- [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) — `sosie.yaml` in detail: possible rule values, `anonymize` vs `pseudonymize`.
- [`CHANGELOG.md`](CHANGELOG.md) — release history.

## Current limitations

- MySQL / MariaDB only, `fr_FR` locale only for generated values.
- No compressed output: pipe it yourself (`sosie transform … | zstd -o out.sql.zst`).

## Development

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

These three commands are the ones run by CI (`.gitlab-ci.yml`).

## License

Licensed under either of MIT or Apache-2.0, at your option. See [`LICENSE.md`](LICENSE.md).
