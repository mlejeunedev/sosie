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
- **Guided setup** — `sosie init` scans the schema *and the content* of your dump and generates a ready-to-review config. Ambiguous columns come with a cautious default rule: you only confirm or change them.
- **Safe by default** — `check` fails in CI if a sensitive column has no rule; `transform` refuses to start as a last resort.
- **Fakes that look real** — IBANs with a valid checksum, well-formed phone numbers, plausible addresses and dates. `UNIQUE` / `PRIMARY KEY` values stay unique, `NOT NULL` columns never get a `NULL`.
- **Anonymize or pseudonymize** — a random key per run (irreversible), or a stable key from `SOSIE_KEY` for reproducible output across exports.
- **Byte-exact passthrough** — everything that isn't transformed is written back unchanged.
- **Data-free reports** — the end-of-run summary only contains counters, never a data value.

## Installation

Sosie is a single command-line binary, `sosie`. There are two ways to get it — pick the one that matches your machine:

| You have… | Use | Time |
|---|---|---|
| Rust installed (`cargo --version` prints a version) | [Option A — `cargo install`](#option-a--you-already-have-rust) | ~1 min (compiles) |
| No Rust, and you don't want to install it | [Option B — prebuilt binary](#option-b--you-dont-have-rust-prebuilt-binary) | ~1 min (download) |

Not sure? Run `cargo --version`. If you get "command not found", take option B.

### Option A — you already have Rust

Sosie is published on [crates.io](https://crates.io/crates/sosie). A recent stable toolchain from [rustup](https://rustup.rs) is enough, on any OS (servers included):

```bash
cargo install --locked sosie
sosie --version
```

`cargo install` puts `sosie` in `~/.cargo/bin`, which rustup already added to your `PATH`: the `sosie` command works from any directory. To upgrade later, run the same `cargo install --locked sosie` again.

### Option B — you don't have Rust (prebuilt binary)

Download the archive for your platform from the [GitHub Releases page](https://github.com/mlejeunedev/sosie/releases):

| Platform | Archive |
|---|---|
| macOS, Apple Silicon (M1, M2…) | `sosie-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `sosie-<version>-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `sosie-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x86_64 | `sosie-<version>-x86_64-pc-windows-msvc.zip` |

**macOS / Linux** — extract, then move the binary to a directory in your `PATH` so `sosie` works from anywhere:

```bash
tar xzf sosie-<version>-<platform>.tar.gz
cd sosie-<version>-<platform>
xattr -d com.apple.quarantine ./sosie   # macOS only, see below
sudo mv sosie /usr/local/bin/
sosie --version
```

- **macOS:** the binary isn't signed, so without the `xattr` line macOS blocks it on first launch ("Apple could not verify…"). The command removes that block once and for all.
- **Linux:** the prebuilt binary needs a recent glibc (Ubuntu 24.04+, Debian 13+ or equivalent). On an older distribution, install Rust and use option A.
- Don't want to touch `/usr/local/bin`? Skip the `mv` line and call the binary by its path instead: `./sosie --version` (and `./sosie init …` in the commands below).

**Windows** — extract the `.zip`, open a terminal in the extracted folder and run `.\sosie.exe --version`. To call `sosie` from anywhere, add that folder to your `PATH` (System settings → Environment variables).

### From source (contributors)

```bash
git clone https://github.com/mlejeunedev/sosie.git
cd sosie
cargo build --release
./target/release/sosie --help
```

## Quick start

The whole workflow, start to finish — three Sosie commands (`init`, `check`, `transform`) and a single manual step:

| Step | Command | What it does | Who works |
|---|---|---|---|
| 1 | `mysqldump` | exports the real database to a SQL file | you, one command |
| 2 | `sosie init` | analyzes the dump and **writes** a starter `sosie.yaml` | Sosie |
| 3 | *(editing)* | you **review** the few columns Sosie couldn't decide alone | **you — the only manual step** |
| 4 | `sosie check` | **verifies** the config covers every sensitive column | Sosie |
| 5 | `sosie transform` | **produces** the anonymized dump | Sosie |
| 6 | `mysql` | imports the anonymized dump locally | you, one command |

**1. Dump the database to anonymize** — plain `mysqldump`, nothing Sosie-specific:

```bash
mysqldump --single-transaction my_db > dump.sql
```

**2. Generate a starter config with `sosie init`** — Sosie reads the table structure *and* a sample of the data, recognizes personal data (emails, names, phones, IBANs, addresses…) and writes a `sosie.yaml` with one rule per sensitive column:

```bash
sosie init --from dump.sql --out sosie.yaml
```

It ends with a summary such as `12 column rule(s), 1 pattern(s), 4 decision(s) to confirm`. The last number is your work for step 3.

**3. Review the config — the human step.** Open `sosie.yaml`. It has four sections where columns are decided, each column in **one place only**:

| Section | Contains | Your job |
|---|---|---|
| `tables:` | columns Sosie is **sure** about, with their rule (`email: email`) | nothing — glance at it |
| `columns:` | rules for several columns at once (`"*.phone": phone`) | nothing — it's also where confirmed `review` lines go |
| `exclude:` | columns to **keep as is**, never anonymized | fill it with the harmless ones |
| `review:` | columns Sosie **isn't sure** about, each with a default rule | **empty it** |

**`review:` is your to-do list.** Each line is a column that *might* hold personal data, with the safest default rule already written (it erases or fakes the value). For each line, decide:

- **It is personal data** → cut the line and paste it under `columns:` (change the rule if you want another one). The rule is now confirmed.
- **It is harmless** (a product name, a catalog description) → put its key in `exclude:` and delete the line. The column will be copied as is.

Generated by `init`:

```yaml
exclude: []

review:
  orders.comments: null              # free text
  products.productName: full_name    # name (name)
```

After your review:

```yaml
columns:
  orders.comments: null              # moved here: customers write phone numbers in it

exclude:
  - products.productName             # a product name, not a person

review: {}
```

When every line is decided, `review: {}` is empty. As long as it isn't, `sosie check` fails — so no guess reaches the output without a human having looked at it (meanwhile, the default rules apply: a forgotten line never leaks data). Same decision for many columns? Use a pattern: `- "*.*Description"` in `exclude:` covers every description column. The possible rules (`keep`, `null`, `constant("…")`, a preset) are described in [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md).

**4. Check that everything is covered with `sosie check`** — re-analyzes the dump and compares it with your config:

```bash
sosie check --from dump.sql --config sosie.yaml
```

`check OK` means every sensitive column is decided, each in one place, and `review:` is empty. Otherwise it lists what's missing and exits with code 1 — fix the config and run it again.

**5. Produce the anonymized dump with `sosie transform`:**

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

`dump_clean.sql` is a valid SQL dump, importable as is, where personal data has been replaced by consistent fake values (same structure, same format, `NOT NULL` constraints respected). Everything else is copied byte for byte.

**6. Import locally**

```bash
mysql my_db_dev < dump_clean.sql
```

Next time the production database changes, keep your `sosie.yaml` (it's meant to be versioned with your code) and only rerun steps 1, 4, 5 and 6: `check` tells you if a new sensitive column appeared.

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
