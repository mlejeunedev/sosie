# Sosie — configuration: rule values and modes

> Technical details about the content of `sosie.yaml`. For the list of commands and the full walkthrough, see `docs/USAGE.md`; for installation and quick start, see the `README.md`.

## The 4 possible values for a column rule

This is the whole grammar accepted for a rule, under `tables.<table>.<column>`, `columns` or `review` (see below) — there is no other.

| Form | Example | Effect |
|---|---|---|
| `keep` | `id: keep` | copied as is, explicitly (silences the scan) |
| `null` | `api_token: null` | emptied (`NULL`, or an empty string if the column is `NOT NULL`) |
| `constant("...")` | `password: constant("x")` | always the same fixed value |
| a preset name, bare or as a map | `email: email` or `{ preset: date_shift, days: 365 }` | plausible, deterministic fake value (see the presets table in `docs/USAGE.md`) |

Two pitfalls to know about:

- **`review` is not a rule value.** It is a section (see below); writing `nickname: review` would be interpreted as an unknown preset name and rejected.
- **Only 16 presets exist** (listed in `docs/USAGE.md`). If a column would need a preset that isn't implemented yet (`company`, `lorem`, `regex_mask`, `json`...), you are limited to `keep`/`null`/`constant` for now.

## Where a column is decided: `tables`, `columns`, `exclude`, `review`

A column is decided in **one place only**:

| Section | Meaning | Form |
|---|---|---|
| `tables` | confirmed rule, for one column | `tables: { user: { email: email } }` |
| `columns` | confirmed rule, for one or several columns | `"*.description": null`, `order.notes: null` |
| `exclude` | column kept as is (never anonymized) | `- products.productName`, `- "*.*Description"` |
| `review` | **not decided yet**: a default rule written by `init`, waiting for you | `products.productName: full_name` |

A key is `<table>.<column>`, split on the first `.`; the only wildcard is `*` (any run of characters, possibly empty). Keys starting with `*` must be quoted, since YAML gives `*` a special meaning. Without `*`, a key designates exactly one column: `order.notes: null` under `columns` is equivalent to `tables: { order: { notes: null } }`.

### `columns` — one rule for several columns

```yaml
columns:
  "*.description": null        # every `description` column, in every table
  "*.*_email": email           # customer_email, billing_email...
  "log_*.*": keep              # every column of the log_* tables
  order.notes: null            # a single column, written flat
```

Between `tables` and `columns`, an exact `tables` rule wins; among `columns` patterns, the **first** matching one in file order applies — put the narrowest first. A broad pattern like `"*.*": keep` silences everything, like `on_unclassified: keep`: use it knowingly.

### `exclude` — keep columns as is, quickly

```yaml
exclude:
  - products.productName       # a product, not a person
  - "*.*Description"           # every description: catalog text
```

Equivalent to a `keep` rule, in a list that is fast to fill: the usual destination of `init`'s false positives.

### `review` — decisions left to you

`init` writes there the columns it isn't sure about, each with a **cautious default rule** (the suggested preset, else `null`, which erases the value):

```yaml
review:
  orders.comments: null              # free text
  products.productName: full_name    # name (name)
```

- These default rules **apply** as long as the entries stay there: a forgotten review never leaks data.
- `check` **fails** as long as `review` isn't empty: every guess must be looked at by a human.
- To decide an entry, **move it**: the whole line to `columns:` to confirm the rule (possibly changing it), or only its key to `exclude:` to keep the column as is.

```yaml
columns:
  orders.comments: null              # confirmed: customers write phone numbers here
exclude:
  - products.productName             # a product name, not a person
review: {}
```

### One place only

A column that is in `review` or `exclude` must not also have a rule in `tables`/`columns` (and not be in both `review` and `exclude`). An identical key in two sections is rejected when the config is loaded; overlapping `*` patterns (`order.notes: null` in `columns` and `"*.notes"` in `exclude`) are reported by `check` and refused by `transform`, which list the columns concerned.

`check` also warns about a pattern that matches no column of the dump (most likely a typo).

## At the whole-table level

Besides per-column rules, two options apply to a whole table:

```yaml
skip_tables:
  - audit_log        # structure kept, zero rows in the output
truncate_tables:
  - session          # same: handy for a table that is entirely at risk
```

Useful when a whole table is at risk rather than handling it column by column — columns of these tables don't need a rule, `check` exempts them automatically.

## `anonymize` or `pseudonymize`?

```yaml
mode: anonymize        # or pseudonymize
```

Both modes apply the same presets; what changes is where the HMAC key that drives the deterministic generation of fake values comes from, and how long it lives.

### `anonymize` (the default)

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

- The key is generated randomly **on every run**, used in memory, then discarded. It is never displayed, never written to disk.
- Two runs on the same dump therefore produce **different** fake values each time.
- Irreversible even in theory: nobody — not even you — can recover the original value from the output.
- This is the mode to use by default, especially for anything that leaves your control (sharing, CI, demo environments).

### `pseudonymize`

```bash
export SOSIE_KEY="a-secret-passphrase-of-at-least-16-characters"
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

- The key comes from the `SOSIE_KEY` environment variable (at least 16 characters), never from the config file (rejected at load time if you try).
- **Deterministic**: same dump + same key + same preset ⇒ always the same fake values. `jean.dupont@gmail.com`, for instance, will always give the same fake email, including in an export made a month later.
- Useful when you need to cross-reference several dumps over time (compare two exports, keep consistency across several dev/staging databases), or keep `user.id=42` recognizable across tables without a declared foreign key.
- If `SOSIE_KEY` is not set, or too short, `check`/`transform` refuse to start with a clear message.

**Warning**: `pseudonymize` output is still personal data under the law (GDPR) — anyone who knows `$SOSIE_KEY` can verify or recover an original value (and even without the key, a dictionary attack on common emails remains possible, since HMAC is deterministic). Handle the resulting dump with the same care as production: don't share it more widely than an `anonymize` dump, and keep `SOSIE_KEY` out of the repository (environment variable, secrets vault — never in plain text in a versioned file).

### Which one to choose?

| Need | Mode |
|---|---|
| One-off copy for local debugging | `anonymize` |
| Sharing a dump with someone external | `anonymize` |
| Comparing two exports taken at different dates | `pseudonymize` |
| Keeping consistency across several databases (dev, staging, CI) without declared foreign keys | `pseudonymize` |
