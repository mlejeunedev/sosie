# Sosie

**Copie une base MySQL de production sur un poste de développeur, sans qu'une seule donnée personnelle réelle ne s'y retrouve.**

Sosie lit un dump `mysqldump`, remplace les colonnes sensibles (email, nom, téléphone, IBAN, adresse...) par des valeurs fictives mais plausibles, et réécrit un dump SQL valide — en streaming, sans jamais charger le fichier entier en mémoire.

```
mysqldump ma_base | sosie transform --config sosie.yaml > dump_clean.sql
mysql ma_base_dev < dump_clean.sql
```

## Pourquoi

Un dump brut de prod sur un laptop de développeur, c'est une fuite de données personnelles qui s'ignore. Les alternatives habituelles sont mauvaises : des fixtures ne ressemblent jamais à la vraie prod, un script SQL maison est lent et vite obsolète, et refaire les mêmes requêtes anonymisées à la main à chaque fois n'est pas tenable. Sosie automatise ça avec un principe simple : **par défaut, l'outil refuse de tourner si une colonne qui ressemble à une donnée personnelle n'a pas de règle explicite.**

## État actuel

Le cœur du projet est implémenté et testé :

- parseur/writer `mysqldump` en streaming, round-trip byte-à-byte vérifié ;
- 13 presets de transformation déterministes (email, noms, téléphone, IBAN, BIC, adresse, date, IP, hash...) ;
- moteur de transformation piloté par une config YAML, avec garde-fou de sécurité ;
- détection automatique des colonnes sensibles par nom et par contenu, pour générer et vérifier la config (`init` / `check`) ;
- rapport de fin d'exécution (compteurs uniquement, jamais une valeur de donnée).

Pas encore fait (finitions, non bloquantes) : sortie compressée, barre de progression, benchmark à grande échelle, binaires de release. Détails, limitations précises et référence complète : **[`docs/USAGE.md`](docs/USAGE.md)**.

## Installation

Nécessite Rust (voir `rust-toolchain.toml` pour la version exacte, installée automatiquement par `rustup`).

```bash
git clone <ce dépôt>
cd sosie
cargo build --release
./target/release/sosie --help
```

## Démarrage rapide

**1. Dumper la base à anonymiser**

```bash
mysqldump --single-transaction ma_base > dump.sql
```

**2. Générer une config de départ** — Sosie analyse le schéma et un échantillon des données, et propose un `sosie.yaml` :

```bash
sosie init --from dump.sql --out sosie.yaml
```

**3. Relire et compléter la config.** `init` couvre automatiquement ce qu'il reconnaît avec confiance (email, téléphone, IBAN...), mais laisse une section `review:` pour les cas ambigus (`notes`, `nickname`, un champ JSON contenant une clé sensible...) — c'est le seul moment où un humain doit trancher :

```yaml
tables:
  order:
    notes: null       # texte libre qui contenait parfois un téléphone -> on vide
  product:
    name: keep          # nom de produit, pas une personne -> faux positif, on garde
```

**4. Vérifier que tout est couvert**

```bash
sosie check --from dump.sql --config sosie.yaml
# check OK : 6 tables, aucune colonne sensible sans règle.
```

**5. Transformer**

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

`dump_clean.sql` est un dump SQL valide, importable tel quel, où les données personnelles ont été remplacées par des valeurs fictives cohérentes (même structure, même format, contraintes `NOT NULL` respectées).

**6. Importer en local**

```bash
mysql ma_base_dev < dump_clean.sql
```

Le détail de chaque commande, le cas d'usage complet et la liste des presets sont dans `docs/USAGE.md`. Les valeurs de règle possibles (`keep`/`null`/`constant`/preset) et le choix entre `anonymize` et `pseudonymize` sont dans `docs/CONFIGURATION.md`.

## Développement

```bash
cargo test           # suite de tests
cargo fmt --check    # formatage
cargo clippy --all-targets -- -D warnings
```

Ces trois commandes sont celles de la CI (`.gitlab-ci.yml`).

- [`docs/USAGE.md`](docs/USAGE.md) — référence d'utilisation complète (commandes, cas d'usage, presets, limitations).
- [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) — détail technique de `sosie.yaml` : valeurs de règle possibles, `anonymize` vs `pseudonymize`.
- [`docs/PLAN.md`](docs/PLAN.md) — plan de construction, étape par étape.
- [`docs/CAHIER_DES_CHARGES.md`](docs/CAHIER_DES_CHARGES.md) — vision cible et fonctionnalités futures.
