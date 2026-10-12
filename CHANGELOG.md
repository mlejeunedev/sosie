# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `siren`, `siret` and `vat` presets: valid Luhn / VAT keys, input formatting kept, and the same fake SIREN shared by a company's SIREN, SIRET and French VAT number.
- `init` now detects these columns by name (`siret`, `siren`, `vat`/`tva` as a whole word, excluding rates and amounts) and by content, and assigns the preset directly instead of leaving them in `review`.
- `columns:` section: rules matching several columns with a `*` glob on `table.column` (`"*.description": null`). An exact `tables` rule takes precedence; otherwise the first matching pattern, in file order, wins. `check` warns about patterns that match no column.
- `init` groups a column name that gets the same rule in at least 3 tables into a single `columns` pattern, and prints a summary (rules, patterns, decisions to confirm).

### Changed

- `init` pre-fills ambiguous (grey-zone) columns with a cautious rule (the suggested preset, else `null`) marked `# REVIEW`, instead of only listing them: confirming a decision now means reviewing a line, not writing it.
- **`check` now fails while `review:` is not empty.** Once the `# REVIEW` rules are checked (switch harmless columns to `keep`), empty the list. Existing configs that kept entries in `review:` must clear them.
- Column names are matched as whole words instead of substrings: `hotel` is no longer a phone (`tel`), `nombre` a last name (`nom`), `is_true` an address (`rue`), `security_level` a social security number (`secu`). Names ending with a metadata word (`_id`, `_type`, `_count`, `_status`, `_verified`, `_at`…) are no longer flagged (`address_id`, `content_type`, `email_verified_at`). Content detection is unchanged.
- IPv6 content detection uses a real address parse: values such as `1:10` or `12:30:00` are no longer taken for IP addresses.

## [0.1.0] - 2026-09-28

First public release.

### Added

- `transform` command: anonymizes or pseudonymizes a `mysqldump` dump in streaming, from a file or stdin, to a file or stdout, without ever loading the dump into memory (only the deduplication state of `UNIQUE` columns grows with the data).
- `init` command: analyzes a dump (column names and a content sample) and generates a starter `sosie.yaml`, with a `review` section for ambiguous columns.
- `check` command: verifies that the config covers every sensitive column of a dump.
- `presets` command: lists available presets with a generated example.
- 13 deterministic presets (`fr_FR` locale): `hash`, `email`, `first_name`, `last_name`, `full_name`, `phone`, `date_shift`, `iban`, `bic`, `address_line`, `city`, `postcode`, `ip`.
- Two modes: `anonymize` (random key on every run) and `pseudonymize` (key read from `SOSIE_KEY`, output stable from one run to the next).
- Per-column rules: `keep`, `null`, `constant("…")` or a preset with parameters; excluded tables via `skip_tables` / `truncate_tables` (structure kept, no rows).
- Safety guards: refuses to run if a sensitive column has no rule, and rejects any secret key (`dsn`, `password`, `key`) in the config file.
- Schema awareness: guaranteed uniqueness of generated values on `PRIMARY KEY` / `UNIQUE` columns, empty string instead of `NULL` on `NOT NULL` columns, `GENERATED` columns ignored.
- Byte-exact rewrite of everything that isn't transformed (byte-for-byte round-trip verified).
- End-of-run report (counters only, never a data value), with per-column detail via `--verbose` and a detailed JSON report.
- Progress bar on stderr (percentage, throughput, ETA, current table), hidden outside a terminal.
- `--dry-run` option: validates the config and the dump without writing the output.
- `gen_dump` example: realistic synthetic dump generator for testing at scale.
- Prebuilt binaries for Linux, macOS (Intel and Apple Silicon) and Windows, published on GitHub Releases.

### Known limitations

- MySQL only, `fr_FR` locale only.
- No compressed output: compress it yourself (`sosie transform … | zstd -o out.sql.zst`).

[0.1.0]: https://github.com/mlejeunedev/sosie/releases/tag/v0.1.0
