# Sosie — usage guide

> This document describes what the code actually does today. For installation and quick start, see the `README.md`; for rule values and modes, see [`CONFIGURATION.md`](CONFIGURATION.md).

## Build and run

```bash
cargo build --release
./target/release/sosie --help
```

Or directly during development: `cargo run -- <command> ...`.

## The 4 commands

### `sosie init` — generate a starter config

Analyzes a dump (schema **and** content, a sample of 200 values per text column) and proposes a commented `sosie.yaml`.

```bash
sosie init --from dump.sql --out sosie.yaml
```

- Columns detected with a confidence ≥ 0.8 (name and/or content matching an email, phone, IBAN, address...) get a preset.
- Ambiguous columns (confidence 0.4–0.8: `notes`, `nickname`, a JSON column containing a sensitive key...) end up in a `review:` section to be decided by hand.
- The generated file is **not** ready to use: it has to be reviewed, and the `review` columns decided.

### `sosie check` — verify that a config is complete

```bash
sosie check --from dump.sql --config sosie.yaml
```

Re-analyzes the dump and compares it with the config file:

- any column detected as sensitive (≥ 0.8) without an explicit rule → failure (exit code 1);
- any grey-zone column (0.4–0.8) without an explicit rule → failure too;
- tables listed in `skip_tables`/`truncate_tables` are exempt (they come out without data anyway);
- `GENERATED` columns are ignored (they never appear in an `INSERT`).

`check` should pass before you run `transform` with confidence — but `transform` also has its own minimal safeguard (see below). Its exit code makes it a natural CI step.

### `sosie transform` — transform the dump

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
# or as a pipe, the way mysqldump would be used:
mysqldump my_db | sosie transform --config sosie.yaml > dump_clean.sql
```

Options:

- `--from <file>`: reads `stdin` by default.
- `--out <file>`: writes to `stdout` by default. With `--out` on a regular file, the write is atomic (temporary file then rename; on error, the temporary file is deleted — never a partial output under the final name). Devices and pipes such as `/dev/null` are written to directly.
- `--dry-run`: does everything (parse, transform) without writing the output.
- `--verbose`: details the final summary column by column (transformed / null / kept / truncated).

Built-in safeguard: if a column has no rule in the config **and** its name matches a sensitive pattern (email, password, iban, phone...), `transform` refuses to start — without needing `check` to have run first. It is a last-resort safety net based on the name only (not the content, which would require replaying the whole dump).

While running: a progress bar on stderr (percentage, throughput, ETA, current table and rows processed) when `--from` is a file, a spinner with bytes read when the input comes from stdin. It is hidden automatically when stderr is not a terminal (CI, redirection).

At the end: a compact summary in the terminal — one overall line, one line per transformed table (rows and number of columns touched), skipped tables, tables output without transformation grouped on one line, and a `⚠` warning only if values had to be truncated. With `--verbose`, the column-by-column detail. A full report is written to `.sosie/last-report.json` — counters only, never a real value. When the dump goes to stdout (no `--out`), the summary is sent to stderr so it never mixes with the SQL.

### `sosie presets` — list available presets

```bash
sosie presets
```

Displays each preset with an input/output example generated on the fly.

## Walkthrough: cloning a `shop` database locally without PII

Context: you work on an online shop (tables `user`, `address`, `order`, `bank_account`, `product`, `audit_log`). You need to reproduce an order bug locally, without blindly copying real customers' data onto your laptop.

**1. Dump production** — nothing Sosie-specific, it's plain `mysqldump`:

```bash
mysqldump --single-transaction shop > dump.sql
```

**2. Generate a starter config**

```bash
sosie init --from dump.sql --out sosie.yaml
```

Sosie scans the schema *and* the data, and produces a YAML file with the obvious columns already covered (`email`, `iban`, `phone`, `birth_date`...) and a `review:` section for what it can't decide on its own:

```yaml
review:
  - audit_log.payload  # JSON containing a sensitive key
  - order.notes  # free text
  - product.name  # name (name)
  - user.nickname  # name (name)
```

**3. Decide the ambiguous cases** — the only moment a human has to think. Edit the generated YAML: `order.notes: null`, `user.nickname: null`, `product.name: keep` (a product name, not a person), and put the whole `audit_log` table in `skip_tables` (its JSON `payload` contains plain-text emails, not worth parsing finely).

**4. Check before running anything**

```bash
$ sosie check --from dump.sql --config sosie.yaml
check OK: 6 tables, no sensitive column without a rule.
```

**5. Transform**

```bash
$ sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
✔ anonymize  16 rows · 6 tables · 0.0s
  address       3 rows · 3 columns
  audit_log     skipped
  bank_account  2 rows · 3 columns
  order         4 rows · 1 column
  user          5 rows · 6 columns
  1 table without transformation: product
  details: .sosie/last-report.json
```

(`sosie transform --verbose` details each column: transformed, null, kept, truncated.)

A real row, before/after:

```
-- before
(1,'jean.dupont@gmail.com','Jean','Dupont','0612345678','1985-03-14','$2y$13$abcd…','tok_a1b2c3d4e5f6','jeanjean',…)
-- after
(1,'claire.andre.504a71@example.org','Theo','Lefebvre','0795958940','1985-08-26','$2y$13$DEVONLY…',NULL,NULL,…)
```

(Here `password` uses a `constant(...)` rule, and `api_token` / `nickname` use `null`. On a `NOT NULL` column, `null` produces an empty string rather than a literal `NULL`, to stay valid SQL on import — see the note under the presets table.)

**6. Import locally**

```bash
mysql shop_dev < dump_clean.sql
```

## The `sosie.yaml` file

```yaml
version: 1

source:
  kind: mysql          # only supported value in v0.1

mode: anonymize         # or pseudonymize (requires $SOSIE_KEY, >= 16 characters)

defaults:
  locale: fr_FR         # only supported value in v0.1
  on_unclassified: fail # or keep, to disable the safeguard

tables:
  user:
    email: email                                   # bare preset
    birth_date: { preset: date_shift, days: 365 }   # preset with parameters
    password: constant("dev-only-hash")             # fixed value
    api_token: null                                 # -> NULL (or empty string if the column is NOT NULL)
    id: keep                                        # copied as is, explicitly

skip_tables:
  - audit_log           # structure kept, zero rows in the output

review: []              # informative, filled by `init`; `check` doesn't rely on it
```

Important points:

- Never any secret in this file (`dsn`, `password`, `key` at the root or under `source` are rejected at load time) — a schema column named `password` is of course still allowed.
- An unknown preset in the config makes loading fail, with a suggestion if the name looks like an existing preset (typo).

For the details of the 4 possible rule values (`keep`/`null`/`constant`/preset) and the difference between `mode: anonymize` and `mode: pseudonymize`: **[`docs/CONFIGURATION.md`](CONFIGURATION.md)**.

## Available presets

| Preset | Behavior |
|---|---|
| `email` | `firstname.lastname.<hex>@example.org`, from embedded fr_FR lists (the hex suffix keeps values unique) |
| `first_name`, `last_name`, `full_name` | Picked from embedded fr_FR lists |
| `phone` | Plausible French number, same format as the input (national or `+33 ...`) |
| `date_shift` | Shifts the date by ±`days` days (365 by default), same output format |
| `iban` | Same country, random BBAN, mod 97 check digits recomputed and valid |
| `bic` | Same country, rest random |
| `address_line` | `12 rue de la Paix`-style, from an embedded list of fr_FR street names |
| `city` | City picked from an embedded fr_FR list |
| `postcode` | Plausible postcode; `keep_department: true` keeps the first 2 digits |
| `ip` | Address in a documentation range (RFC 5737 / 2001:db8::) |
| `hash` | Hex of the HMAC, for opaque identifiers |

Rules common to all presets: `NULL` stays `NULL`, an empty string stays empty, the output is truncated to the column size (`VARCHAR(n)`), and the same input value always gives the same output for the same preset and the same key (determinism).

Non-preset rules: `keep` (explicit copy), `null` (forces `NULL`, or an empty string if the column is `NOT NULL` — never an invalid literal `NULL`), `constant("...")` (fixed value).

## Known limitations

- MySQL / MariaDB only (no Postgres), `fr_FR` locale only.
- No compressed output: the output file is raw SQL, to compress yourself if needed (`sosie transform ... | zstd -o out.sql.zst`).
- A value escaped in SQL with doubled quotes (`''`, very rare — `mysqldump` always uses `\'`) is parsed correctly but always rewritten in the standard `mysqldump` format (`\'`): the round-trip is identical in content, not necessarily byte for byte in that specific case.
- No Doctrine/Symfony mapping (planned beyond v0.1).

## Testing at scale

A synthetic dump generator is provided as a Cargo example. It produces a realistic `mysqldump` dump (classicmodels schema + a `user` table, varied data, consistent foreign keys, unique emails), deterministic for a given seed, at roughly 200 MB/s:

```
cargo run --release --example gen_dump -- --size 1G --out fixtures/big/big.sql
sosie check --from fixtures/big/big.sql --config sosie.yaml
sosie transform --from fixtures/big/big.sql --config sosie.yaml --out fixtures/big/big.anon.sql
```

`fixtures/big/` is ignored by git. Options: `--size` (`500M`, `1G`, `4G`…), `--seed` (same seed = same dump).

There is no size limit: the dump is processed in streaming, one statement at a time, with a few MB of base memory. The only cost that grows with the data is the deduplication state of `UNIQUE`/`PRIMARY KEY` columns transformed by a preset (e.g. `user.email`): about 150 bytes per distinct value, i.e. ~1.5 GB for 10 million unique emails. Tables in `skip_tables`/`truncate_tables` are skipped without parsing their rows. Measured order of magnitude: 1.1 GB and 17 million rows in ~30 s, under 100 MB peak memory, on an Apple M1 Pro laptop.
