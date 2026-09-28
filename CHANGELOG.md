# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
