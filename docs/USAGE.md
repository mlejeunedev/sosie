# Sosie — usage guide

> This document describes what the code actually does today. For installation and quick start, see the `README.md`; for rule values and modes, see [`CONFIGURATION.md`](CONFIGURATION.md).

## Installing

See the [Installation section of the README](../README.md#installation): `cargo install --locked sosie` if you have Rust, a prebuilt binary otherwise. Contributors can also run from a checkout with `cargo run -- <command> ...`.

## The commands at a glance

| Command | What it's for | Reads | Writes | Run it… |
|---|---|---|---|---|
| [`sosie init`](#sosie-init--generate-a-starter-config) | Propose a `sosie.yaml` for your schema | a dump | `sosie.yaml` | once, at setup |
| [`sosie check`](#sosie-check--verify-that-a-config-is-complete) | Verify the config covers every sensitive column | a dump + the config | nothing | after every config edit, in CI, before each `transform` |
| [`sosie transform`](#sosie-transform--produce-the-anonymized-dump) | Produce the anonymized dump | a dump + the config | the clean dump | every time you need a fresh dump |
| [`sosie presets`](#sosie-presets--list-available-presets) | Show the fake-value generators you can use in rules | nothing | nothing | while editing the config |

Between `init` and `check` sits the only manual step: [reviewing the generated config](#reviewing-the-generated-config-the-human-step).

`sosie --help` and `sosie <command> --help` print the options of each command.

### `sosie init` — generate a starter config

**What it's for:** writing a `sosie.yaml` by hand for a schema of dozens of tables would be long and error-prone. `init` does the bulk of it: it reads the dump (table structure **and** a sample of 200 values per text column), recognizes columns holding personal data, and writes a commented config with a rule for each of them.

**Usage:**

```bash
sosie init --from dump.sql --out sosie.yaml
```

| Option | Required | Meaning |
|---|---|---|
| `--from <file>` | yes | the `mysqldump` file to analyze |
| `--out <file>` | no (default `sosie.yaml`) | the config file to write — **overwritten** if it exists |

**What it produces:** each column gets a confidence score, from its name and its content.

- **Sure (score ≥ 0.8)** — a name or content that clearly matches an email, a phone number, an IBAN, an address, a password… The column gets its rule with no further ado: `email: email  # name`.
- **Unsure (score 0.4–0.8)** — free text (`notes`, `comments`), a generic `name`, a JSON column containing a sensitive key… Sosie can't decide alone, so it writes **a default rule** — the cautious one: the suggested preset if there is one, otherwise `null` (value erased) — marks the line `# REVIEW: <reason>`, and adds the column to the `review:` section at the end of the file.
- **Neutral (score < 0.4)** — ids, prices, dates, statuses… Not written: copied as is.

Two details that keep the file short:

- A column name that gets the same rule in at least 3 tables is written once, as a `columns` pattern: `"*.phone": phone  # name (3 tables)`.
- Names are matched as whole words: `tel` matches `tel_portable` but not `hotel`; metadata columns (`address_id`, `content_type`, `email_verified_at`) are not flagged.

The terminal ends with a summary, e.g. `12 column rule(s), 1 pattern(s), 4 decision(s) to confirm`.

**What you do next:** the file is **not** ready to use yet — `check` refuses it as long as `review:` isn't empty. Go through the [review step](#reviewing-the-generated-config-the-human-step) below.

### Reviewing the generated config (the human step)

This is the only moment where a human has to think, and it's on purpose: Sosie never decides alone that an ambiguous column is harmless.

**The `review:` section is your to-do list.** It lists the columns where *you* have to decide. Each of them already has a rule, written higher up in the file by `init` — under `tables:` (or `columns:` for a pattern) — on a line ending with `# REVIEW`. That rule is a safe default, not a final decision.

Example of what `init` generates:

```yaml
tables:
  order:
    customer_email: email    # name                <- sure: nothing to do
    notes: null              # REVIEW: free text   <- guessed: to confirm
  product:
    name: full_name          # REVIEW: name (name) <- guessed: to confirm

review:                      # <- your to-do list: one entry per # REVIEW line above
  - order.notes  # free text
  - product.name  # name (name)
```

For each entry of `review:`:

1. **Find its `# REVIEW` line** above (`order.notes` → table `order`, column `notes`).
2. **Decide.** Ask yourself: "can this column contain something about a real person?"
   - Yes, or not sure → keep the default rule (it erases or fakes the value), or pick a more fitting one (`constant("…")`, another preset — see [`CONFIGURATION.md`](CONFIGURATION.md)).
   - No (a product name, a technical label) → replace the rule by `keep`.
   - The whole table is at risk and not useful locally → put the table in `skip_tables` instead.
3. **Delete the entry** from `review:`.

Result for the example above:

```yaml
tables:
  order:
    customer_email: email    # name
    notes: null              # customers write their phone number here: default kept
  product:
    name: keep               # a product name, not a person

review: []                   # everything decided
```

Good to know:

- You may delete the `# REVIEW` comments or leave them: Sosie ignores comments. Only the `review:` list counts.
- **An exact line always wins over a pattern.** If you add `columns: { "*.description": keep }` to settle all descriptions at once, also delete the `description: null  # REVIEW` lines from `tables:` — otherwise they still apply to their columns.
- Emptying `review:` without reading the rules makes `check` pass, but defeats its purpose: the defaults are cautious, not necessarily right (a product `name` would be replaced by a person's name).

### `sosie check` — verify that a config is complete

**What it's for:** making sure no sensitive column slips through — after you edited the config, and every time the database schema changes (a new `phone` column added by a migration has no rule yet). Its exit code (0 = OK, 1 = problem) makes it a natural CI step.

**Usage:**

```bash
sosie check --from dump.sql --config sosie.yaml
```

| Option | Required | Meaning |
|---|---|---|
| `--from <file>` | yes | the dump to analyze (the same kind of dump you will transform) |
| `--config <file>` | yes | the `sosie.yaml` to verify |

**What it checks:** it re-analyzes the dump like `init`, then compares with the config. It fails (exit code 1) if:

- a column detected as sensitive (score ≥ 0.8) has no rule;
- an unsure column (score 0.4–0.8) has no rule;
- the `review:` list isn't empty (decisions not yet confirmed).

A rule counts whether it comes from `tables` or from a `columns` pattern. Tables in `skip_tables`/`truncate_tables` are exempt (they come out without data anyway), and `GENERATED` columns are ignored (they never appear in an `INSERT`). A `columns` pattern that matches no column of the dump triggers a warning (likely a typo), without failing.

**What you see:**

```bash
$ sosie check --from dump.sql --config sosie.yaml
check OK: 6 tables, no sensitive column without a rule.
```

or, when something is missing, the list of columns concerned and what to do:

```
Sensitive columns without a rule (score >= 0.8):
  user.phone  — name
2 pre-filled decision(s) to confirm (lines marked `# REVIEW` in the config):
  order.notes
  product.name

check FAILED.
```

**What you do next:** add the missing rules / finish the review, and run `check` again until it says `OK`.

### `sosie transform` — produce the anonymized dump

**What it's for:** the actual job. It reads the original dump, applies the config's rules column by column, and writes a new dump where personal data is replaced by fake but plausible values. Everything the config doesn't touch is copied byte for byte.

**Usage:**

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
# or as a pipe, straight from mysqldump, without an intermediate file:
mysqldump my_db | sosie transform --config sosie.yaml > dump_clean.sql
```

| Option | Required | Meaning |
|---|---|---|
| `--config <file>` | yes | the `sosie.yaml` to apply |
| `--from <file>` | no (default: stdin) | the original dump |
| `--out <file>` | no (default: stdout) | the anonymized dump. On a regular file the write is atomic: a temporary file is renamed at the end, and deleted on error — never a partial output under the final name. Devices and pipes such as `/dev/null` are written to directly. |
| `--dry-run` | no | does everything (parse, transform) without writing the output — handy to validate a config on a big dump |
| `--verbose` | no | details the final summary column by column (transformed / null / kept / truncated) |

**Built-in safeguard:** if a column has no rule **and** its name looks sensitive (email, password, iban, phone...), `transform` refuses to start, even if `check` was never run. It's a last-resort net based on names only — run `check` for the full analysis. It does not look at `review:`: the rules written by `init` apply as they are, which is why `check` must pass first.

**What you see:** while running, a progress bar on stderr (percentage, throughput, ETA, current table and rows) when `--from` is a file, a spinner with bytes read when reading stdin; hidden automatically when stderr is not a terminal (CI, redirection). At the end, a compact summary — one overall line, one line per transformed table (rows and number of columns touched), skipped tables, tables copied without transformation grouped on one line, and a `⚠` warning only if values had to be truncated. A full report is written to `.sosie/last-report.json` — counters only, never a real value. When the dump goes to stdout (no `--out`), the summary goes to stderr so it never mixes with the SQL.

**What you do next:** import the result (`mysql my_db_dev < dump_clean.sql`).

### `sosie presets` — list available presets

**What it's for:** while editing the config, seeing which fake-value generators exist and what they produce.

```bash
sosie presets
```

Prints each preset with an input → output example generated on the fly. The full list with their parameters is in [Available presets](#available-presets) below.

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

Sosie scans the schema *and* the data, and produces a YAML file with the obvious columns already covered (`email`, `iban`, `phone`, `birth_date`...). For what it can't decide on its own, it writes a default rule marked `# REVIEW`, and lists the column in `review:` — your to-do list:

```yaml
tables:
  audit_log:
    payload: null  # REVIEW: JSON containing a sensitive key
  order:
    notes: null  # REVIEW: free text
  product:
    name: full_name  # REVIEW: name (name)
  user:
    nickname: full_name  # REVIEW: name (name)
    ...

review:
  - audit_log.payload  # JSON containing a sensitive key
  - order.notes  # free text
  - product.name  # name (name)
  - user.nickname  # name (name)
```

**3. Confirm the ambiguous cases** — the only moment a human has to think (see [Reviewing the generated config](#reviewing-the-generated-config-the-human-step)). Going through the 4 entries of `review:`:

- `order.notes: null` — customers write phone numbers in it: the default is right, nothing to change.
- `product.name: keep` — a product name, not a person: the default `full_name` would replace "Wireless mouse" by "Jean Martin", so switch it to `keep`.
- `user.nickname: null` — a nickname can identify someone and is useless locally: erase it rather than fake it.
- `audit_log.payload` — its JSON contains plain-text emails, not worth parsing finely: delete the `payload` line and put the whole `audit_log` table in `skip_tables`.

Then empty the list: `review: []`.

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

columns:                # one rule for several columns; an exact `tables` rule wins
  "*.description": null
  "*.*_email": email

skip_tables:
  - audit_log           # structure kept, zero rows in the output

review: []              # pre-filled decisions to confirm, filled by `init`; must be empty for `check` to pass
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
| `siren` | 9-digit SIREN with a valid Luhn key, same formatting as the input (`732 829 320`) |
| `siret` | 14-digit SIRET with a valid Luhn key; its first 9 digits are the fake SIREN of the original company, so sites of one company stay grouped |
| `vat` | French VAT number `FRkk` + fake SIREN with a recomputed key (consistent with `siren`/`siret`); other countries keep their prefix, digits replaced |
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
