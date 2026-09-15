# Sosie — Plan de construction détaillé (v0.1)

Objectif de la v0.1 : `sosie transform` fonctionne sur un vrai dump `mysqldump`, avec `init` et `check`. Tout ce qui n'est pas listé ici attend la v0.2.

Chaque étape a un **critère de sortie** : tant qu'il n'est pas vert, on ne passe pas à la suivante. Les durées sont indicatives pour des sessions de soirée/week-end.

---

## Étape 0 — Squelette du projet (1 soirée)

- [ ] `../Cargo.toml` avec les dépendances de base (clap, anyhow, thiserror, serde, serde_yaml, hmac, sha2, rand_chacha, rand, memchr ; dev : insta, assert_cmd).
- [ ] `../src/lib.rs` qui déclare les modules : `dump`, `config`, `transform`, `presets`, `scan`, `report`.
- [ ] `../src/main.rs` avec clap et les sous-commandes vides : `transform`, `init`, `check`, `presets`. Chacune affiche "not implemented" et retourne un code d'erreur.
- [ ] `../fixtures` : copier `examples/dumps/*.sql` et `examples/configs/*.yaml`.
- [ ] CI GitHub Actions : `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.
- [ ] `rust-toolchain.toml` pour figer la version.

**Sortie** : `cargo run -- transform` compile, affiche l'aide, `cargo test` passe (0 tests).

---

## Étape 1 — Le modèle d'événements (1 soirée)

Fichier : `src/dump/mod.rs`

- [ ] `pub struct Column { name: String, sql_type: SqlType, nullable: bool, max_len: Option<u32>, generated: bool }`
- [ ] `pub enum SqlType { Int, Decimal, Float, Char, Text, Blob, Date, DateTime, Json, Enum, Set, Bit, Other(String) }`
- [ ] `pub struct Table { name: String, columns: Vec<Column> }`
- [ ] `pub enum Value<'a> { Null, Raw(&'a [u8]), Str(Cow<'a, [u8]>) }`
  - `Raw` = tout ce qui n'est pas une chaîne SQL (nombres, `0x…`, `_binary '…'`, `b'…'`, `NULL` traité à part). Copié tel quel, jamais transformé.
  - `Str` = contenu **désescapé** d'une chaîne `'…'`. Le sérialiseur ré-échappe.
- [ ] `pub enum Event<'a> { Raw(&'a [u8]), TableSchema(Table), RowsBegin { table: String, columns: Option<Vec<String>> }, Row(Vec<Value<'a>>), RowsEnd }`
  - `RowsBegin.columns` : `Some` si l'`INSERT` a une liste de colonnes explicite (cas des colonnes générées).
- [ ] `pub trait DumpParser { fn next_event(&mut self) -> Result<Option<Event<'_>>> }`
- [ ] `pub trait DumpWriter { fn write_event(&mut self, ev: &Event) -> Result<()> }`

**Sortie** : ça compile, et les types sont documentés (`///`) parce que tu les reliras dans trois mois.

---

## Étape 2 — Parseur `mysqldump` : le round-trip (3 à 4 soirées, le cœur du projet)

Fichier : `src/dump/mysql.rs`

### 2a. Lecture par blocs
- [ ] Lire `stdin`/fichier avec un `BufReader` de 1 Mo et un buffer interne qui grandit uniquement si une instruction ne tient pas (un `INSERT` étendu peut faire 16 Mo).
- [ ] Découper le flux en **instructions** terminées par `;` suivi de fin de ligne, en tenant compte des chaînes (`'…'`, `"…"`), des commentaires (`-- …`, `/* … */`, `/*!40101 … */`) et de `DELIMITER ;;`.
- [ ] Tout ce qui n'est pas `CREATE TABLE` ni `INSERT INTO` → `Event::Raw`.

### 2b. `CREATE TABLE`
- [ ] Extraire le nom (backticks, éventuellement `schema`.`table`).
- [ ] Pour chaque ligne de définition commençant par un backtick : nom, type, longueur `(n)`, `NOT NULL`, `GENERATED ALWAYS`.
- [ ] Ignorer `PRIMARY KEY`, `KEY`, `UNIQUE`, `CONSTRAINT`, `CHECK`, `FULLTEXT` — sauf mémoriser les `FOREIGN KEY` dans `Table.foreign_keys` (utile en v0.2, coût nul maintenant).
- [ ] L'instruction complète est **aussi** conservée en `Raw` pour être réécrite à l'identique : on ne régénère jamais un `CREATE TABLE`.

### 2c. `INSERT INTO`
- [ ] Nom de table, liste de colonnes optionnelle `(\`a\`, \`b\`)`.
- [ ] Tokenizer de `VALUES` : machine à états `Outside`, `InSingleQuote`, `Escape`, `InHex`, `InBinaryPrefix`. Séparateurs `,` et tuples `( … )`.
- [ ] Désescaper : `\'`, `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\Z`, `\b`, `''`.
- [ ] Émettre `RowsBegin`, puis un `Row` par tuple, puis `RowsEnd`.
- [ ] Le préfixe `INSERT INTO \`t\` VALUES ` est conservé pour réécriture à l'identique.

### 2d. Sérialiseur
- [ ] `Raw` → écriture brute.
- [ ] `Row` : ré-échapper les `Str` avec exactement les règles de `mysqldump` (il n'échappe que `\`, `'`, `"`, `\n`, `\r`, `\0`, `\Z`, `\t` ? → à vérifier empiriquement sur un vrai dump, c'est ce que le round-trip va révéler).
- [ ] Reconstituer `(…),(…),…;` avec les mêmes séparateurs.

### 2e. Tests
- [ ] `tests/roundtrip.rs` : pour chaque fichier de `fixtures/dumps/`, parse → write → `assert_eq!(bytes)`. Utiliser `similar-asserts` ou afficher l'offset du premier octet différent.
- [ ] Test unitaire par cas d'échappement (une fonction `unescape` / `escape` testées en round-trip).
- [ ] `proptest` : générer des chaînes aléatoires, `escape(unescape(escape(s))) == escape(s)`.
- [ ] Générer un vrai dump avec un MySQL local (Docker) contenant des valeurs tordues, l'ajouter aux fixtures. Ne fais pas confiance à mes fixtures écrites à la main : `mysqldump` a ses propres habitudes.

**Sortie** : T01 vert sur `01_basic.sql`, `02_parser_edge_cases.sql` et un dump réel. Mémoire < 50 Mo sur un dump de 1 Go (générer avec un script).

---

## Étape 3 — Configuration (1 à 2 soirées)

Fichier : `src/config.rs`

- [ ] Structs serde : `Config { version, source, mode, defaults, tables: BTreeMap<String, BTreeMap<String, Rule>>, skip_tables, truncate_tables, review, sampling }`.
- [ ] `Rule` désérialisée depuis soit une chaîne (`email`, `null`, `keep`, `constant("…")`), soit une map (`{ preset: date_shift, days: 365 }`). Implémenter un `Deserialize` custom ou un `enum` `#[serde(untagged)]`.
- [ ] Validation après chargement : preset inconnu → erreur avec suggestion ("did you mean `first_name`?"), `dsn`/`password`/`key` présents → erreur, `mode: pseudonymize` sans `$SOSIE_KEY` → erreur.
- [ ] Tests : charger chaque fixture yaml ; un yaml invalide par type d'erreur.

**Sortie** : `Config::load("sosie.yaml")` sur les 5 fixtures ; erreurs lisibles.

---

## Étape 4 — Moteur de transformation et premiers presets (3 soirées)

Fichiers : `src/transform.rs`, `src/presets/`

### 4a. Le trait et la graine
- [ ] `trait Preset { fn apply(&self, input: &Value, seed: &Seed, ctx: &ColumnCtx) -> Value; fn name(&self) -> &str; }`
- [ ] `Seed` = 32 octets = `HMAC-SHA256(key, preset_name || 0x00 || valeur_brute)`. Fonction `seed_for(key, preset, value)`.
- [ ] `Seed::rng() -> ChaCha8Rng` pour piocher dans des listes.
- [ ] Clé : en mode `anonymize`, `rand::random::<[u8; 32]>()` au démarrage, jamais loggée ; en `pseudonymize`, `$SOSIE_KEY` (au moins 16 caractères).

### 4b. Presets P0, dans cet ordre
1. [ ] `keep`, `null`, `constant` (triviaux, valident la plomberie).
2. [ ] `hash` (hex du seed, tronqué à `max_len`).
3. [ ] `email` : `prenom.nom@example.org`, listes embarquées via `include_str!("data/fr_FR/first_names.txt")`.
4. [ ] `first_name`, `last_name`, `full_name`.
5. [ ] `phone` : détecter le format d'entrée (E.164 `+33…` vs national `06…`), produire le même format.
6. [ ] `date_shift` : parser `YYYY-MM-DD[ HH:MM:SS]`, décaler de `±days` dérivé du seed, réémettre au même format.
7. [ ] `iban` : garder les 2 lettres pays, générer un BBAN de la bonne longueur (table pays → longueur), calculer la clé mod 97. Test : la sortie passe une validation IBAN indépendante.
8. [ ] `address_line`, `city`, `postcode` (avec `keep_department`).

Règles transverses testées pour **chaque** preset : `Null → Null`, chaîne vide → chaîne vide, troncature à `max_len` comptée dans le rapport, déterminisme (même seed → même sortie).

### 4c. Le plan par table
- [ ] À chaque `TableSchema` : construire `Vec<Option<Box<dyn Preset>>>` indexé par position de colonne. Gérer `RowsBegin.columns` (liste explicite) en remappant les positions.
- [ ] Tables dans `skip_tables` : émettre le `CREATE TABLE`, avaler les `Row`.
- [ ] Sur `Row` : pour chaque valeur avec un preset, remplacer ; sinon passer.

### 4d. Le garde-fou
- [ ] **Avant d'écrire le premier octet**, faire une première passe légère ? Non : un dump sur stdin ne se relit pas. Solution : la vérification se fait sur les `CREATE TABLE` au fur et à mesure, mais comme `mysqldump` émet le `CREATE TABLE` juste avant ses `INSERT`, on peut avoir déjà écrit d'autres tables. Donc :
  - `check` (étape 6) est la vraie barrière, à lancer avant.
  - `transform` en plus **refuse** dès qu'il rencontre une table avec une colonne sensible non couverte (via `scan` sur le nom), s'arrête, supprime le fichier de sortie partiel s'il l'a créé lui-même, et code retour ≠ 0.
  - Écriture dans un fichier temporaire + `rename` atomique à la fin : jamais de sortie partielle sous le nom final.

### 4e. Tests
- [ ] Golden `insta` : `01_basic.sql` + `03_pseudonymize.yaml` + `SOSIE_KEY=test-…` → snapshot.
- [ ] Assertions de T02 (script `run.sh` ou en Rust avec `assert_cmd`).

**Sortie** : T02, T03 (partie transform), T04, T05 verts.

---

## Étape 5 — Rapport (1 soirée)

Fichier : `src/report.rs`

- [ ] Compteurs : lignes lues/écrites par table, colonnes transformées/null/keep, troncatures, tables skippées, objets `Raw` notables (VIEW, TRIGGER, PROCEDURE), durée, débit, mémoire max (`/proc/self/status` sur Linux, sinon absent).
- [ ] Affichage terminal (tableau simple) + écriture `.sosie/last-report.json`.
- [ ] Aucune valeur de données dans le rapport, testé.

**Sortie** : T02 assertion 17 verte.

---

## Étape 6 — `scan`, `init`, `check` (3 soirées)

Fichier : `src/scan.rs`

### 6a. Détection par nom
- [ ] Table de motifs → preset + score (voir cahier des charges §5.5). Normaliser le nom : minuscule, `camelCase` → `snake_case`.
- [ ] Exclusions (`table_name`, `file_name`, `class_name`, `role_name`, `product.name`…) → score réduit.

### 6b. Détection par contenu
- [ ] Sur un dump : pendant le parse, garder les 200 premières valeurs non nulles de chaque colonne texte.
- [ ] Regex email, IBAN (+ mod 97), téléphone, IP ; ratio ≥ 0,8 → score fort.
- [ ] Texte long contenant email/téléphone dans ≥ 5 % des valeurs → `review`.
- [ ] Colonne JSON dont les clés matchent 6a → `review`.

### 6c. `init`
- [ ] Combiner les scores, produire le YAML avec commentaires (`# nom + contenu`) : générer le texte à la main plutôt que via serde pour garder les commentaires.
- [ ] Test T06 : comparer sémantiquement à `01_basic.expected-init.yaml`.

### 6d. `check`
- [ ] Recalculer le scan, comparer à la config : colonnes ≥ 0,8 sans règle, `review` non vide, `on_unclassified: keep` sans flag → liste + code retour 1.
- [ ] Test T03 (partie check).

**Sortie** : T03, T06 verts. `sosie presets` liste les presets avec un exemple généré.

---

## Étape 7 — Finitions v0.1 (2 soirées)

- [ ] Sortie `.sql.zst` (`zstd` crate, encoder en flux) et `.sql.gz`.
- [ ] `--dry-run` : tout sauf l'écriture.
- [x] Barre de progression (`indicatif`) si stdin est un fichier de taille connue.
- [ ] Messages d'erreur : toujours `table.colonne` + numéro d'instruction, jamais de contenu.
- [ ] README : le chrono des 10 minutes, un GIF, le tableau des presets, "ce que Sosie garantit / ne garantit pas".
- [ ] Bench : dump synthétique 1 Go, mesurer débit et mémoire, mettre les chiffres dans le README.
- [ ] `cargo publish --dry-run`, release GitHub avec binaires Linux/macOS (cross via `cargo-dist`).

**Sortie** : un inconnu installe Sosie, suit le README et transforme un dump en 10 minutes. Demande à un collègue de le faire sans ton aide : c'est le vrai test.

---

## Ordre de bataille résumé

```
0 squelette ─▶ 1 événements ─▶ 2 parseur + round-trip ─▶ 3 config
                                                            │
7 finitions ◀─ 6 scan/init/check ◀─ 5 rapport ◀─ 4 transform + presets
```

Étapes 2 et 4 représentent 70 % du travail et 100 % de la valeur. Si tu bloques, c'est là, et c'est là qu'on regarde ensemble.

---

## Ce qu'on ne fait PAS en v0.1 (pour résister à la tentation)

Postgres, connexion directe, sampling, `pull` via SSH, Doctrine, chiffrement, preset `json`, parallélisme, bundle Symfony, site web. Chacun est noté dans le cahier des charges avec sa priorité ; aucun ne rend la v0.1 plus utile.
