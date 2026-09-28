# Sosie — Detailed build plan (v0.1)

Goal of v0.1: `sosie transform` works on a real `mysqldump` dump, with `init` and `check`. Everything not listed here waits for v0.2.

Each step has an **exit criterion**: as long as it isn't green, we don't move on to the next one. Durations are indicative, for evening/weekend sessions.

---

## Step 0 — Project skeleton (1 evening)

- [ ] `../Cargo.toml` with the base dependencies (clap, anyhow, thiserror, serde, serde_yaml, hmac, sha2, rand_chacha, rand, memchr; dev: insta, assert_cmd).
- [ ] `../src/lib.rs` declaring the modules: `dump`, `config`, `transform`, `presets`, `scan`, `report`.
- [ ] `../src/main.rs` with clap and empty subcommands: `transform`, `init`, `check`, `presets`. Each prints "not implemented" and returns an error code.
- [ ] `../fixtures`: copy `examples/dumps/*.sql` and `examples/configs/*.yaml`.
- [ ] GitHub Actions CI: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.
- [ ] `rust-toolchain.toml` to pin the version.

**Exit**: `cargo run -- transform` compiles, prints the help, `cargo test` passes (0 tests).

---

## Step 1 — The event model (1 evening)

File: `src/dump/mod.rs`

- [ ] `pub struct Column { name: String, sql_type: SqlType, nullable: bool, max_len: Option<u32>, generated: bool }`
- [ ] `pub enum SqlType { Int, Decimal, Float, Char, Text, Blob, Date, DateTime, Json, Enum, Set, Bit, Other(String) }`
- [ ] `pub struct Table { name: String, columns: Vec<Column> }`
- [ ] `pub enum Value<'a> { Null, Raw(&'a [u8]), Str(Cow<'a, [u8]>) }`
  - `Raw` = everything that isn't an SQL string (numbers, `0x…`, `_binary '…'`, `b'…'`, `NULL` handled separately). Copied as is, never transformed.
  - `Str` = **unescaped** content of a `'…'` string. The serializer re-escapes it.
- [ ] `pub enum Event<'a> { Raw(&'a [u8]), TableSchema(Table), RowsBegin { table: String, columns: Option<Vec<String>> }, Row(Vec<Value<'a>>), RowsEnd }`
  - `RowsBegin.columns`: `Some` if the `INSERT` has an explicit column list (the generated-columns case).
- [ ] `pub trait DumpParser { fn next_event(&mut self) -> Result<Option<Event<'_>>> }`
- [ ] `pub trait DumpWriter { fn write_event(&mut self, ev: &Event) -> Result<()> }`

**Exit**: it compiles, and the types are documented (`///`) because you'll reread them in three months.

---

## Step 2 — `mysqldump` parser: the round-trip (3 to 4 evenings, the heart of the project)

File: `src/dump/mysql.rs`

### 2a. Block reading
- [ ] Read `stdin`/file with a 1 MB `BufReader` and an internal buffer that only grows if a statement doesn't fit (an extended `INSERT` can be 16 MB).
- [ ] Split the stream into **statements** terminated by `;` followed by a line ending, taking into account strings (`'…'`, `"…"`), comments (`-- …`, `/* … */`, `/*!40101 … */`) and `DELIMITER ;;`.
- [ ] Everything that isn't `CREATE TABLE` or `INSERT INTO` → `Event::Raw`.

### 2b. `CREATE TABLE`
- [ ] Extract the name (backticks, possibly `schema`.`table`).
- [ ] For each definition line starting with a backtick: name, type, length `(n)`, `NOT NULL`, `GENERATED ALWAYS`.
- [ ] Ignore `PRIMARY KEY`, `KEY`, `UNIQUE`, `CONSTRAINT`, `CHECK`, `FULLTEXT` — except record `FOREIGN KEY`s in `Table.foreign_keys` (useful in v0.2, zero cost now).
- [ ] The full statement is **also** kept as `Raw` to be rewritten identically: we never regenerate a `CREATE TABLE`.

### 2c. `INSERT INTO`
- [ ] Table name, optional column list `(\`a\`, \`b\`)`.
- [ ] `VALUES` tokenizer: state machine `Outside`, `InSingleQuote`, `Escape`, `InHex`, `InBinaryPrefix`. Separators `,` and tuples `( … )`.
- [ ] Unescape: `\'`, `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\Z`, `\b`, `''`.
- [ ] Emit `RowsBegin`, then one `Row` per tuple, then `RowsEnd`.
- [ ] The `INSERT INTO \`t\` VALUES ` prefix is kept to be rewritten identically.

### 2d. Serializer
- [ ] `Raw` → raw write.
- [ ] `Row`: re-escape `Str` values with exactly `mysqldump`'s rules (does it only escape `\`, `'`, `"`, `\n`, `\r`, `\0`, `\Z`, `\t`? → to be checked empirically on a real dump, which is what the round-trip will reveal).
- [ ] Rebuild `(…),(…),…;` with the same separators.

### 2e. Tests
- [ ] `tests/roundtrip.rs`: for each file in `fixtures/dumps/`, parse → write → `assert_eq!(bytes)`. Use `similar-asserts` or print the offset of the first differing byte.
- [ ] One unit test per escaping case (an `unescape` / `escape` function tested in round-trip).
- [ ] `proptest`: generate random strings, `escape(unescape(escape(s))) == escape(s)`.
- [ ] Generate a real dump with a local MySQL (Docker) containing twisted values, add it to the fixtures. Don't trust my hand-written fixtures: `mysqldump` has its own habits.

**Exit**: T01 green on `01_basic.sql`, `02_parser_edge_cases.sql` and a real dump. Memory < 50 MB on a 1 GB dump (generated with a script).

---

## Step 3 — Configuration (1 to 2 evenings)

File: `src/config.rs`

- [ ] Serde structs: `Config { version, source, mode, defaults, tables: BTreeMap<String, BTreeMap<String, Rule>>, skip_tables, truncate_tables, review, sampling }`.
- [ ] `Rule` deserialized from either a string (`email`, `null`, `keep`, `constant("…")`) or a map (`{ preset: date_shift, days: 365 }`). Implement a custom `Deserialize` or a `#[serde(untagged)]` `enum`.
- [ ] Validation after loading: unknown preset → error with a suggestion ("did you mean `first_name`?"), `dsn`/`password`/`key` present → error, `mode: pseudonymize` without `$SOSIE_KEY` → error.
- [ ] Tests: load each yaml fixture; one invalid yaml per error type.

**Exit**: `Config::load("sosie.yaml")` on the 5 fixtures; readable errors.

---

## Step 4 — Transformation engine and first presets (3 evenings)

Files: `src/transform.rs`, `src/presets/`

### 4a. The trait and the seed
- [ ] `trait Preset { fn apply(&self, input: &Value, seed: &Seed, ctx: &ColumnCtx) -> Value; fn name(&self) -> &str; }`
- [ ] `Seed` = 32 bytes = `HMAC-SHA256(key, preset_name || 0x00 || raw_value)`. Function `seed_for(key, preset, value)`.
- [ ] `Seed::rng() -> ChaCha8Rng` to pick from lists.
- [ ] Key: in `anonymize` mode, `rand::random::<[u8; 32]>()` at startup, never logged; in `pseudonymize`, `$SOSIE_KEY` (at least 16 characters).

### 4b. P0 presets, in this order
1. [ ] `keep`, `null`, `constant` (trivial, they validate the plumbing).
2. [ ] `hash` (hex of the seed, truncated to `max_len`).
3. [ ] `email`: `firstname.lastname@example.org`, lists embedded via `include_str!("data/fr_FR/first_names.txt")`.
4. [ ] `first_name`, `last_name`, `full_name`.
5. [ ] `phone`: detect the input format (E.164 `+33…` vs national `06…`), produce the same format.
6. [ ] `date_shift`: parse `YYYY-MM-DD[ HH:MM:SS]`, shift by `±days` derived from the seed, re-emit in the same format.
7. [ ] `iban`: keep the 2 country letters, generate a BBAN of the right length (country → length table), compute the mod 97 check digits. Test: the output passes an independent IBAN validation.
8. [ ] `address_line`, `city`, `postcode` (with `keep_department`).

Cross-cutting rules tested for **each** preset: `Null → Null`, empty string → empty string, truncation to `max_len` counted in the report, determinism (same seed → same output).

### 4c. The per-table plan
- [ ] On each `TableSchema`: build a `Vec<Option<Box<dyn Preset>>>` indexed by column position. Handle `RowsBegin.columns` (explicit list) by remapping positions.
- [ ] Tables in `skip_tables`: emit the `CREATE TABLE`, swallow the `Row`s.
- [ ] On `Row`: for each value with a preset, replace it; otherwise pass it through.

### 4d. The safeguard
- [ ] **Before writing the first byte**, do a light first pass? No: a dump on stdin can't be re-read. Solution: the check happens on each `CREATE TABLE` as we go, but since `mysqldump` emits the `CREATE TABLE` right before its `INSERT`s, other tables may already have been written. So:
  - `check` (step 6) is the real barrier, to be run beforehand.
  - `transform` additionally **refuses** as soon as it meets a table with an uncovered sensitive column (via `scan` on the name), stops, deletes the partial output file if it created it itself, and exits with a non-zero code.
  - Write to a temporary file + atomic `rename` at the end: never a partial output under the final name.

### 4e. Tests
- [ ] `insta` golden test: `01_basic.sql` + `03_pseudonymize.yaml` + `SOSIE_KEY=test-…` → snapshot.
- [ ] T02 assertions (a `run.sh` script or in Rust with `assert_cmd`).

**Exit**: T02, T03 (transform part), T04, T05 green.

---

## Step 5 — Report (1 evening)

File: `src/report.rs`

- [ ] Counters: rows read/written per table, columns transformed/null/keep, truncations, skipped tables, notable `Raw` objects (VIEW, TRIGGER, PROCEDURE), duration, throughput, peak memory (`/proc/self/status` on Linux, absent otherwise).
- [ ] Terminal display (simple table) + writing `.sosie/last-report.json`.
- [ ] No data value in the report, tested.

**Exit**: T02 assertion 17 green.

---

## Step 6 — `scan`, `init`, `check` (3 evenings)

File: `src/scan.rs`

### 6a. Name-based detection
- [ ] Pattern table → preset + score (see the specification §5.5). Normalize the name: lowercase, `camelCase` → `snake_case`.
- [ ] Exclusions (`table_name`, `file_name`, `class_name`, `role_name`, `product.name`…) → reduced score.

### 6b. Content-based detection
- [ ] On a dump: while parsing, keep the first 200 non-null values of each text column.
- [ ] Email, IBAN (+ mod 97), phone, IP regexes; ratio ≥ 0.8 → strong score.
- [ ] Long text containing an email/phone in ≥ 5% of values → `review`.
- [ ] JSON column whose keys match 6a → `review`.

### 6c. `init`
- [ ] Combine the scores, produce the YAML with comments (`# name + content`): generate the text by hand rather than via serde to keep the comments.
- [ ] Test T06: compare semantically with `01_basic.expected-init.yaml`.

### 6d. `check`
- [ ] Recompute the scan, compare with the config: columns ≥ 0.8 without a rule, non-empty `review`, `on_unclassified: keep` without a flag → list + exit code 1.
- [ ] Test T03 (check part).

**Exit**: T03, T06 green. `sosie presets` lists the presets with a generated example.

---

## Step 7 — v0.1 polish (2 evenings)

- [ ] `.sql.zst` output (`zstd` crate, streaming encoder) and `.sql.gz`.
- [ ] `--dry-run`: everything except writing.
- [x] Progress bar (`indicatif`) if stdin is a file of known size.
- [ ] Error messages: always `table.column` + statement number, never content.
- [ ] README: the 10-minute timeline, a GIF, the presets table, "what Sosie guarantees / doesn't guarantee".
- [ ] Bench: 1 GB synthetic dump, measure throughput and memory, put the numbers in the README.
- [ ] `cargo publish --dry-run`, GitHub release with Linux/macOS binaries (cross via `cargo-dist`).

**Exit**: a stranger installs Sosie, follows the README and transforms a dump in 10 minutes. Ask a colleague to do it without your help: that's the real test.

---

## Battle order, summarized

```
0 skeleton ─▶ 1 events ─▶ 2 parser + round-trip ─▶ 3 config
                                                      │
7 polish ◀─ 6 scan/init/check ◀─ 5 report ◀─ 4 transform + presets
```

Steps 2 and 4 represent 70% of the work and 100% of the value. If you get stuck, it's there, and that's where we look together.

---

## What we DON'T do in v0.1 (to resist temptation)

Postgres, direct connection, sampling, `pull` over SSH, Doctrine, encryption, `json` preset, parallelism, Symfony bundle, website. Each one is listed in the specification with its priority; none of them makes v0.1 more useful.
