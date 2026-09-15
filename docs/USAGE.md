# Sosie — état actuel et guide d'utilisation

> Ce document décrit ce que fait réellement le code aujourd'hui (pas la vision cible : voir `CAHIER_DES_CHARGES.md`, ni le plan de construction : voir `PLAN.md`).

## Où on en est

Les étapes 0 à 6 du `PLAN.md` sont faites et fonctionnelles de bout en bout :

- **Parseur/writer `mysqldump`** (`src/dump/mysql.rs`) : découpe un dump en évènements (`CREATE TABLE`, lignes d'un `INSERT`, reste tel quel), round-trip byte-à-byte vérifié par tests.
- **Config** (`src/config`) : chargement et validation de `sosie.yaml`.
- **Presets** (`src/presets`) : 13 transformateurs déterministes (voir plus bas).
- **Moteur de transformation** (`src/transform`) : applique la config à un dump, en streaming, avec garde-fou de sécurité.
- **Rapport** (`src/report`) : compteurs de fin d'exécution (jamais de valeur de donnée).
- **Détection** (`src/scan`) : classification des colonnes par nom + par contenu, pour `init` et `check`.

**Pas encore fait** (étape 7 du plan) : sortie compressée (`.zst`/`.gz`), mode `--dry-run` réellement testé à grande échelle, benchmark 1 Go, packaging/release binaires. Rien de tout ça n'empêche d'utiliser l'outil aujourd'hui.

## Compiler et lancer

```bash
cargo build --release
./target/release/sosie --help
```

Ou directement en développement : `cargo run -- <commande> ...`.

## Les 4 commandes

### `sosie init` — générer une config de départ

Analyse un dump (schéma **et** contenu, échantillon de 200 valeurs par colonne texte) et propose un `sosie.yaml` commenté.

```bash
sosie init --from dump.sql --out sosie.yaml
```

- Les colonnes détectées avec une confiance ≥ 0,8 (nom et/ou contenu correspondant à un email, téléphone, IBAN, adresse...) reçoivent un preset.
- Les colonnes ambiguës (confiance 0,4–0,8 : `notes`, `nickname`, une colonne JSON contenant une clé sensible...) atterrissent dans une section `review:` à trancher à la main.
- Le fichier généré n'est **pas** prêt à l'emploi : il faut le relire et décider quoi faire des colonnes en `review`.

### `sosie check` — vérifier qu'une config est complète

```bash
sosie check --from dump.sql --config sosie.yaml
```

Ré-analyse le dump et compare au fichier de config :

- toute colonne détectée comme sensible (≥ 0,8) sans règle explicite → échec (code de sortie 1) ;
- toute colonne en zone grise (0,4–0,8) sans règle explicite → échec aussi ;
- les tables listées dans `skip_tables`/`truncate_tables` sont exemptées (elles sortiront de toute façon sans données) ;
- les colonnes `GENERATED` sont ignorées (elles n'apparaissent jamais dans un `INSERT`).

`check` doit passer avant de lancer `transform` en confiance — mais `transform` a aussi son propre garde-fou minimal (voir plus bas).

### `sosie transform` — transformer le dump

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
# ou en pipe, comme mysqldump le ferait :
mysqldump ma_base | sosie transform --config sosie.yaml > dump_clean.sql
```

Options :

- `--from <fichier>` : par défaut, lit `stdin`.
- `--out <fichier>` : par défaut, écrit sur `stdout`. Avec `--out`, l'écriture est atomique (fichier temporaire puis renommage ; en cas d'erreur, le fichier temporaire est supprimé, jamais de sortie partielle sous le nom final).
- `--dry-run` : fait tout (parse, transforme) sans écrire la sortie.

Garde-fou intégré : si une colonne n'a pas de règle dans la config **et** que son nom correspond à un motif sensible (email, password, iban, phone...), `transform` refuse de démarrer — sans avoir besoin d'avoir lancé `check` avant. C'est une sécurité de dernier recours basée sur le nom seul (pas le contenu, qui nécessiterait de rejouer tout le dump).

Pendant l'exécution : une barre de progression sur stderr (pourcentage, débit, ETA, table courante et lignes traitées) si `--from` est un fichier, un spinner avec les octets lus si l'entrée vient de stdin. Elle est masquée automatiquement quand stderr n'est pas un terminal (CI, redirection).

À la fin : un résumé sur le terminal (lignes/colonnes traitées par table) et un rapport détaillé écrit dans `.sosie/last-report.json` — uniquement des compteurs, jamais une valeur réelle. Quand le dump part sur stdout (pas de `--out`), le résumé est envoyé sur stderr pour ne jamais se mélanger au SQL.

### `sosie presets` — lister les presets disponibles

```bash
sosie presets
```

Affiche chaque preset avec un exemple d'entrée/sortie généré à la volée.

## Cas d'usage : cloner une base `shop` en local sans PII

Contexte : tu bosses sur une boutique en ligne (tables `user`, `address`, `order`, `bank_account`, `product`, `audit_log`). Tu dois reproduire un bug de commande en local, sans copier bêtement les données de vrais clients sur ton laptop.

**1. Dump de la prod** — rien de spécial à Sosie, c'est du `mysqldump` classique :

```bash
mysqldump --single-transaction shop > dump.sql
```

**2. Générer une config de départ**

```bash
sosie init --from dump.sql --out sosie.yaml
```

Sosie scanne le schéma *et* les données, et produit un YAML avec les colonnes évidentes déjà couvertes (`email`, `iban`, `phone`, `birth_date`...) et une section `review:` pour ce qu'il ne peut pas trancher seul :

```yaml
review:
  - audit_log.payload  # JSON contenant une clé sensible
  - order.notes         # texte libre
  - product.name        # nom (name)
  - user.nickname        # nom (name)
```

**3. Trancher les cas ambigus** — le seul moment où un humain doit réfléchir. On édite le YAML généré : `order.notes: null`, `user.nickname: null`, `product.name: keep` (nom de produit, pas une personne), et toute la table `audit_log` dans `skip_tables` (son `payload` JSON contient des emails en clair, pas envie de le parser finement).

**4. Vérifier avant de lancer quoi que ce soit**

```bash
$ sosie check --from dump.sql --config sosie.yaml
check OK : 6 tables, aucune colonne sensible sans règle.
```

**5. Transformer**

```bash
$ sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
sosie transform — terminé en 0.0s
  user — 5 lignes
    email: 5 transformées, first_name: 5, last_name: 5, password: 5, phone: 4/1 null...
  audit_log — skippée (structure gardée, 0 ligne)
```

Une ligne réelle, avant/après :

```
-- avant
(1,'jean.dupont@gmail.com','Jean','Dupont','0612345678','1985-03-14','$2y$13$...',...)
-- après
(1,'baptiste.david@example.org','Damien','Girard','0793816898','1985-01-24','',...)
```

(`password` est `NOT NULL` dans le schéma : la règle `null` produit une chaîne vide, pas un `NULL` littéral, pour rester du SQL valide à l'import — voir la note sous le tableau des presets.)

**6. Importer en local**

```bash
mysql shop_dev < dump_clean.sql
```

## Le fichier `sosie.yaml`

```yaml
version: 1

source:
  kind: mysql          # seule valeur supportée en v0.1

mode: anonymize         # ou pseudonymize (nécessite $SOSIE_KEY, >= 16 caractères)

defaults:
  locale: fr_FR         # seule valeur supportée en v0.1
  on_unclassified: fail # ou keep, pour désactiver le garde-fou

tables:
  user:
    email: email                                   # preset nu
    birth_date: { preset: date_shift, days: 365 }   # preset avec paramètres
    password: constant("dev-only-hash")             # valeur fixe
    api_token: null                                 # -> NULL (ou chaîne vide si colonne NOT NULL)
    id: keep                                        # copie telle quelle, explicite

skip_tables:
  - audit_log           # structure gardée, zéro ligne dans la sortie

review: []              # informatif, rempli par `init` ; `check` ne s'y fie pas
```

Points importants :

- Jamais de secret dans ce fichier (`dsn`, `password`, `key` à la racine ou sous `source` sont refusés au chargement) — une colonne de schéma nommée `password` reste bien sûr autorisée.
- Un preset inconnu dans la config fait échouer le chargement, avec une suggestion si le nom ressemble à un preset existant (faute de frappe).

Pour le détail des 4 valeurs de règle possibles (`keep`/`null`/`constant`/preset) et la différence entre `mode: anonymize` et `mode: pseudonymize` : **[`docs/CONFIGURATION.md`](CONFIGURATION.md)**.

## Les presets disponibles

| Preset | Comportement |
|---|---|
| `email` | `prenom.nom@example.org`, à partir de listes fr_FR embarquées |
| `first_name`, `last_name`, `full_name` | Piochés dans des listes fr_FR embarquées |
| `phone` | Numéro français plausible, même format que l'entrée (national ou `+33 ...`) |
| `date_shift` | Décale la date de ±`days` jours (365 par défaut), même format en sortie |
| `iban` | Même pays, BBAN aléatoire, clé de contrôle mod 97 recalculée et valide |
| `bic` | Même pays, reste aléatoire |
| `address_line` | `12 rue de la Paix`-style, liste de voies fr_FR embarquée |
| `city` | Ville tirée d'une liste fr_FR embarquée |
| `postcode` | Code postal plausible ; `keep_department: true` conserve les 2 premiers chiffres |
| `ip` | Adresse dans une plage documentation (RFC 5737 / 2001:db8::) |
| `hash` | Hex du HMAC, pour les identifiants opaques |

Règles communes à tous les presets : `NULL` reste `NULL`, une chaîne vide reste vide, la sortie est tronquée à la taille de la colonne (`VARCHAR(n)`), et une même valeur d'entrée donne toujours la même sortie pour un même preset et une même clé (déterminisme).

Rules hors preset : `keep` (copie explicite), `null` (force `NULL`, ou chaîne vide si la colonne est `NOT NULL` — jamais de `NULL` littéral invalide), `constant("...")` (valeur fixe).

## Limitations connues

- MySQL uniquement (pas Postgres), locale `fr_FR` uniquement.
- Pas de sortie compressée : le fichier de sortie est du SQL brut, à compresser soi-même si besoin (`sosie transform ... | zstd -o out.sql.zst`).
- Une valeur échappée en SQL avec des apostrophes doublées (`''`, rarissime — `mysqldump` utilise toujours `\'`) est comprise correctement mais toujours ré-écrite au format `mysqldump` standard (`\'`) : le round-trip est donc identique en contenu, pas forcément octet pour octet sur ce cas précis.
- Pas de mapping Doctrine/Symfony (prévu au-delà de la v0.1).

## Tester à grande échelle

Un générateur de dump synthétique est fourni en exemple Cargo. Il produit un dump `mysqldump` réaliste (schéma classicmodels + table `user`, données variées, clés étrangères cohérentes, emails uniques), déterministe pour une graine donnée, à environ 200 Mo/s :

```
cargo run --release --example gen_dump -- --size 1G --out fixtures/big/big.sql
sosie check --from fixtures/big/big.sql --config sosie.yaml
sosie transform --from fixtures/big/big.sql --config sosie.yaml --out fixtures/big/big.anon.sql
```

`fixtures/big/` est ignoré par git. Options : `--size` (`500M`, `1G`, `4G`…), `--seed` (même graine = même dump).

Il n'y a pas de limite de taille : le dump est traité en streaming, une instruction à la fois, avec quelques Mo de mémoire de base. Le seul coût qui grandit avec les données est l'état de déduplication des colonnes `UNIQUE`/`PRIMARY KEY` transformées par un preset (ex. `user.email`) : environ 150 octets par valeur distincte, soit ~1,5 Go pour 10 millions d'emails uniques. Les tables dans `skip_tables`/`truncate_tables` sont sautées sans parser leurs lignes. Ordre de grandeur mesuré : 1 Go et 17 millions de lignes en 40 s sur un portable.
