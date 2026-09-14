# dbclone — Cahier des charges

> Copier une base de production sur un laptop de développeur, sans exposer une seule donnée client réelle. En moins de 10 minutes de configuration.

Version : 0.1 (brouillon de travail)  
Langage : Rust (édition 2024)  
Licence envisagée : Apache-2.0 ou MIT (le cœur), version hébergée propriétaire plus tard  
Nom de code : `dbclone` (à valider, vérifier la disponibilité sur crates.io / GitHub / domaine)

---

## 1. Vision et objectifs

### 1.1 Le problème

Tout développeur qui travaille sur une application avec de vrais utilisateurs finit par avoir besoin des données de production en local : reproduire un bug, tester une migration, vérifier des performances sur des volumes réels. Aujourd'hui, les options sont toutes mauvaises :

| Option | Problème |
|---|---|
| Fixtures / données générées | Ne ressemblent jamais à la prod, ne reproduisent pas les cas bizarres |
| Dump brut de la prod | Données personnelles réelles sur des laptops : violation RGPD, risque de fuite |
| Scripts SQL d'anonymisation maison | Lents (heures sur des dizaines de millions de lignes), incomplets, jamais maintenus, cassent la cohérence entre tables |
| Outils existants | Postgres-only, nécessitent un serveur à déployer, ou abandonnés (Replibyte : dernière release 2022, 118 issues ouvertes) |

### 1.2 Objectif principal

**Objectif n°1 : permettre à un développeur de récupérer une copie fidèle de la base de production sur sa machine, dans laquelle aucune donnée permettant d'identifier une personne réelle ne subsiste.**

### 1.3 Objectifs secondaires

1. **Configuration en moins de 10 minutes** sur un projet de 50 tables, sans lire de documentation.
2. **Sûr par défaut** : l'outil refuse de tourner si une colonne détectée comme sensible n'a pas de règle explicite. L'oubli est impossible par construction.
3. **Les vraies données ne quittent jamais la prod** : la transformation s'exécute là où les données sont, seul le résultat anonymisé voyage.
4. **Hyper rapide** : streaming pur, mémoire constante, capable de traiter 60 millions de lignes / 32 Go sans mettre le serveur à genoux.
5. **Cohérence préservée** : un même client transformé donne les mêmes fausses valeurs dans toutes les tables et d'un dump à l'autre.
6. **Compréhension de Symfony/Doctrine** : génération de la configuration à partir du mapping des entités (différenciateur).

### 1.4 Indicateur de succès du MVP

> Un développeur qui n'a jamais vu l'outil, sur un projet Symfony de 50 tables et une base de 5 Go, obtient une base locale fonctionnelle (l'application démarre et affiche des clients fictifs cohérents) en moins de 10 minutes de travail humain.

### 1.5 Non-objectifs (v0.x)

- Ne remplace pas un outil de backup/restauration.
- Ne fait pas de synchronisation continue / CDC.
- Ne gère pas MongoDB, SQLite, SQL Server, Oracle (peut-être plus tard).
- Ne fournit pas de garantie juridique d'anonymisation au sens strict (k-anonymat, differential privacy). Il fournit une pseudonymisation robuste et une anonymisation pratique, documentée par un rapport.
- Pas d'interface graphique en v0.x.

---

## 2. Vocabulaire

| Terme | Définition retenue |
|---|---|
| **Anonymisation** | Transformation irréversible. Sans la clé, impossible de retrouver la valeur d'origine. Mode par défaut (clé aléatoire jetée après exécution). |
| **Pseudonymisation** | Transformation déterministe avec une clé conservée. Même valeur d'entrée → même valeur de sortie, à chaque exécution. Permet de recouper des dumps dans le temps. La donnée reste "personnelle" au sens RGPD : à traiter comme telle. |
| **Transformateur** | Fonction qui remplace la valeur d'une colonne (`email`, `iban`, `null`…). |
| **Preset** | Transformateur nommé, prêt à l'emploi, avec locale (`phone_fr`, `address_fr`). |
| **Scan** | Analyse du schéma et d'un échantillon de données pour détecter les colonnes sensibles. |
| **Échantillonnage (sampling)** | Réduction du volume en ne gardant qu'une fraction des lignes "racines" (ex. 5 % des clients) et toutes les lignes qui leur sont liées. |
| **Source** | D'où viennent les données : dump sur stdin/fichier, ou connexion directe (réplica). |
| **Cible** | Où va le résultat : fichier, stdout, ou base locale. |

---

## 3. Personas et cas d'usage

### Persona A — Le développeur Symfony (cible principale)
Travaille sur une appli e-commerce / SaaS avec MySQL ou Postgres. A besoin d'une base réaliste une fois par semaine. N'a pas envie d'apprendre un nouvel outil : veut une commande.

### Persona B — Le lead / DevOps
Doit fournir des environnements de review et de staging avec des données crédibles, sans violer le RGPD. Veut un binaire dans le pipeline CI et un rapport à montrer au DPO.

### Persona C — La fintech / la boîte réglementée
Manipule IBAN, montants, identités. A l'obligation de prouver que les données de non-prod sont anonymisées. Veut du déterminisme (pseudonymisation), de la traçabilité, du format-preserving.

### Cas d'usage prioritaires

1. `dbclone pull` hebdomadaire par un dev (A).
2. Génération d'un environnement de review à chaque PR (B).
3. Rapport de conformité "aucune colonne sensible non traitée" (B, C).
4. Reproduction d'un bug client précis sans voir l'identité du client (A, C).

---

## 4. Parcours utilisateur cible (le chrono des 10 minutes)

```
 0:00  brew install dbclone            # ou curl | sh, ou scp du binaire sur le serveur
 1:00  dbclone init --from mysql://ro@replica/app --doctrine src/Entity
       → inspecte le schéma, échantillonne 200 lignes/table, lit les entités
       → génère dbclone.yaml (uniquement les colonnes sensibles + une section "review")
 3:00  relecture de dbclone.yaml, arbitrage des 2-3 colonnes en "review"
 6:00  dbclone pull --sample 5%
       → dump côté serveur → transform en flux → compression → transfert → restore local
       → affiche le rapport : lignes traitées, colonnes couvertes, durée
 8:00  symfony serve → connexion → clients fictifs cohérents
10:00  git add dbclone.yaml && git commit
```

Tout ce qui n'est pas sur ce chemin critique n'est pas dans la v0.1.

---

## 5. Fonctionnalités

Légende priorité : **P0** = MVP v0.1 obligatoire · **P1** = v0.2 · **P2** = v0.3 · **P3** = plus tard.

### 5.1 Commandes CLI

| Commande | Priorité | Description |
|---|---|---|
| `dbclone transform` | P0 | Lit un dump (stdin ou fichier), applique la config, écrit un dump transformé. Le cœur de l'outil. |
| `dbclone init` | P0 | Scanne une source (dump ou connexion) et génère `dbclone.yaml`. |
| `dbclone check` | P0 | Vérifie que la config couvre toutes les colonnes sensibles détectées. Code retour non nul sinon. Pensé pour la CI. |
| `dbclone restore` | P1 | Importe un dump transformé dans une base locale (ou lance un conteneur Docker et importe dedans). |
| `dbclone pull` | P1 | Enchaîne dump distant (via SSH) → transform → compression → transfert → restore. La commande "une seule commande". |
| `dbclone sample` | P1 | Sous-échantillonnage cohérent par clés étrangères (peut être une option de `transform` plutôt qu'une commande). |
| `dbclone presets` | P0 | Liste les transformateurs disponibles avec un exemple de sortie. |
| `dbclone report` | P1 | Ré-affiche / exporte (JSON, Markdown) le rapport de la dernière exécution. |
| `dbclone diff-schema` | P2 | Compare le schéma actuel à celui vu au dernier `init` et signale les nouvelles colonnes non classées. |

### 5.2 Sources et cibles

| Fonction | Priorité |
|---|---|
| Source : dump `mysqldump` (format INSERT multi-valeurs, avec ou sans `--extended-insert`) | P0 |
| Source : dump `pg_dump` format plain (`COPY ... FROM stdin`) | P1 |
| Source : connexion directe MySQL (lecture par curseur, `--single-transaction`) | P1 |
| Source : connexion directe Postgres | P2 |
| Source : dump distant via SSH (`ssh host 'mysqldump ...' \| dbclone transform`) | P1 |
| Cible : fichier `.sql`, `.sql.zst`, `.sql.gz`, stdout | P0 |
| Cible : base locale par connexion | P1 |
| Cible : conteneur Docker créé à la volée | P2 |
| Compression zstd en flux | P0 |
| Chiffrement AES-256-GCM en flux (age ou équivalent) | P2 |

### 5.3 Fichier de configuration `dbclone.yaml`

Principes :
- **Court** : on ne liste que les colonnes sensibles. Tout ce qui n'est pas listé est copié tel quel (sauf si le scan l'a marqué sensible, auquel cas `check` échoue).
- **Presets, pas DSL** : la valeur d'une colonne est un nom de preset. Les options sont rares.
- **Versionnable** : vit dans le repo, relu en PR.

```yaml
version: 1

source:
  kind: mysql          # mysql | postgres
  # dsn: lu depuis $DBCLONE_SOURCE_DSN, jamais écrit ici

mode: anonymize        # anonymize | pseudonymize
# key: lu depuis $DBCLONE_KEY en mode pseudonymize. En mode anonymize, clé aléatoire non conservée.

defaults:
  locale: fr_FR
  on_unclassified: fail   # fail | null | keep   (keep interdit sauf --i-know-what-i-do)

tables:
  user:
    email: email
    first_name: first_name
    last_name: last_name
    phone: phone
    birth_date: { preset: date_shift, days: 365 }
    password: constant("$2y$13$invalidhashfordevonly")
    api_token: null

  address:
    line1: address_line
    line2: null
    city: city
    postcode: postcode
    country: keep          # explicite : on garde

  order:
    customer_email: email  # même preset que user.email → même sortie pour même entrée
    notes: null            # texte libre : on ne prend pas de risque
    amount: keep

  bank_account:
    iban: iban
    bic: bic
    holder_name: full_name

skip_tables:               # tables copiées vides (structure seulement)
  - audit_log
  - messenger_messages
  - sessions

truncate_tables:           # tables totalement ignorées (pas même la structure) — rare
  - cache_items

review:                    # rempli par init, doit être vide pour que check passe
  - user.nickname          # détecté "peut-être sensible" (nom + contenu ambigu)

sampling:                  # P1
  root: user
  fraction: 0.05
  always_keep:
    - "user.id IN (1, 2, 3)"   # comptes de test internes
```

### 5.4 Transformateurs (presets)

Tous les presets sont **déterministes** : `sortie = f(HMAC-SHA256(clé, valeur_entrée))`. Deux colonnes avec le même preset et la même valeur d'entrée produisent la même sortie. Les presets marqués ⚑ préservent le format (la sortie passe les mêmes validateurs que l'entrée).

| Preset | Priorité | Comportement | Exemple |
|---|---|---|---|
| `email` ⚑ | P0 | Prénom.nom dérivés du hash + domaine dans une liste réservée (`example.org`, `example.net`). Conserve la casse d'origine ? Non : lowercase. | `jean.dupont@gmail.com` → `marc.lefevre@example.org` |
| `first_name` | P0 | Prénom tiré d'une liste locale (fr_FR : ~500 prénoms) indexée par le hash | `Jean` → `Marc` |
| `last_name` | P0 | Idem, noms de famille | `Dupont` → `Lefèvre` |
| `full_name` | P0 | Prénom + nom | |
| `phone` ⚑ | P0 | Numéro plausible au format local (fr : `06`/`07` + 8 chiffres, ou format E.164 si l'entrée l'était). Utilise les plages réservées à la fiction quand elles existent. | `0612345678` → `0698765432` |
| `null` | P0 | Met `NULL` (ou chaîne vide si `NOT NULL`, détecté au schéma) | |
| `constant(...)` | P0 | Valeur fixe | mot de passe de dev |
| `keep` | P0 | Copie telle quelle, mais **explicite** (fait taire le scan) | |
| `hash` | P0 | Hex du HMAC, tronqué à la taille de la colonne. Pour les identifiants opaques. | |
| `iban` ⚑ | P0 | IBAN syntaxiquement valide (clé de contrôle mod 97 correcte), même pays que l'entrée, BBAN aléatoire | `FR76 3000 6000 0112 3456 7890 189` → `FR76 1234 5678 9012 3456 7890 123` |
| `bic` ⚑ | P1 | Code BIC plausible, même pays | |
| `address_line` | P0 | Numéro + type de voie + nom de voie (fr) | `12 rue de la Paix` → `47 avenue des Tilleuls` |
| `city` | P0 | Ville tirée d'une liste locale | |
| `postcode` ⚑ | P0 | Code postal plausible ; option : même département que l'entrée | `75001` → `75014` |
| `date_shift` | P0 | Décalage déterministe de ±N jours (défaut 365). Préserve l'ordre relatif ? Non, mais préserve la plausibilité. | |
| `date_year_only` | P1 | Garde l'année, met 01-01 | |
| `number_noise` | P1 | ±N % déterministe, garde l'ordre de grandeur (montants, salaires) | |
| `company` | P1 | Raison sociale fictive | |
| `siret` ⚑ / `siren` ⚑ | P1 | Numéro à clé de Luhn valide | |
| `ip` | P1 | IP dans une plage documentation (`192.0.2.0/24`) | |
| `url` | P1 | `https://example.org/<hash>` | |
| `lorem(n)` | P1 | Texte libre remplacé par du lorem de longueur similaire | |
| `json` | P2 | Applique des presets à des clés d'un champ JSON (`{ path: "$.email", preset: email }`) | |
| `regex_mask` | P2 | Masque partiel : `**** **** **** 1234` | |
| `custom` (WASM) | P3 | Transformateur utilisateur en WASM | |

Règles transverses :
- Une valeur `NULL` en entrée reste `NULL` en sortie (sauf `constant`).
- Une chaîne vide reste vide.
- La sortie est tronquée à la longueur de la colonne (`VARCHAR(n)`) et l'événement est compté dans le rapport.
- Les presets respectent la locale par défaut, surchargeable par colonne : `{ preset: phone, locale: en_GB }`.

### 5.5 Détection (`init` et `check`)

Trois signaux, combinés en un score par colonne. Seuils : ≥ 0,8 → sensible (règle obligatoire) ; 0,4–0,8 → `review` ; < 0,4 → ignoré.

**Signal 1 — Nom de colonne** (poids fort). Liste de motifs (insensible à la casse, snake/camel, fr/en) :

| Motifs | Preset proposé |
|---|---|
| `email`, `mail`, `courriel` | `email` |
| `first_name`, `firstname`, `prenom`, `given_name` | `first_name` |
| `last_name`, `lastname`, `nom`, `surname`, `family_name` | `last_name` |
| `name`, `full_name`, `display_name`, `holder` (hors `table_name`, `file_name`, `class_name`, `role_name`…) | `full_name` ou `review` |
| `phone`, `tel`, `mobile`, `telephone` | `phone` |
| `iban`, `bban`, `account_number` | `iban` |
| `bic`, `swift` | `bic` |
| `address`, `adresse`, `street`, `rue`, `line1`, `line2` | `address_line` |
| `city`, `ville`, `town` | `city` |
| `zip`, `postcode`, `postal_code`, `code_postal` | `postcode` |
| `birth`, `dob`, `naissance` | `date_shift` |
| `ssn`, `nir`, `secu`, `social_security` | `null` |
| `password`, `passwd`, `pwd`, `hash`, `token`, `secret`, `api_key`, `salt` | `constant` / `null` |
| `ip`, `ip_address`, `remote_addr` | `ip` |
| `note`, `comment`, `message`, `body`, `content`, `description` | `review` (texte libre : risque de PII incluse) |
| `siret`, `siren`, `vat`, `tva` | `siret` / `siren` |

**Signal 2 — Contenu** (échantillon de 200 lignes non nulles par colonne, poids moyen) :
- ≥ 80 % des valeurs matchent une regex email → `email`
- ≥ 80 % matchent `^[A-Z]{2}\d{2}[A-Z0-9]{11,30}$` et passent mod 97 → `iban`
- ≥ 80 % matchent un numéro de téléphone (E.164 ou format local) → `phone`
- ≥ 80 % matchent une IP → `ip`
- Colonne texte longue (> 100 caractères en moyenne) contenant des emails/téléphones dans ≥ 5 % des valeurs → `review`
- Colonne JSON dont les clés matchent le signal 1 → `review` + suggestion `json`

**Signal 3 — Mapping Doctrine** (`--doctrine src/Entity`, poids fort si présent) :
- Attributs `#[ORM\Column]` : nom de propriété et type (`string`, `text`, `json`, `date`).
- Lecture des annotations/attributs de validation Symfony : `#[Assert\Email]`, `#[Assert\Iban]`, `#[Assert\Bic]`, `#[Assert\Regex]` sur un téléphone… → preset direct, score 1,0.
- Classes implémentant `UserInterface` / `PasswordAuthenticatedUserInterface` → `password` en `constant`, identifiant en `email`.
- Relations `#[ORM\ManyToOne]` / `OneToMany` → graphe de clés étrangères pour le sampling (P1), utile quand la base n'a pas de contraintes FK déclarées.
- Le parsing PHP se limite aux attributs et propriétés : un tokenizer léger suffit, pas un parseur PHP complet (à valider ; sinon `mago-syntax` ou `php-parser-rs`).

**Sortie de `init`** : un `dbclone.yaml` avec les colonnes ≥ 0,8 pré-remplies, les 0,4–0,8 en `review`, et un commentaire par ligne indiquant le signal qui a déclenché (`# nom + contenu`, `# Assert\Email`).

**`check`** échoue si : `review` non vide ; une colonne ≥ 0,8 sans règle ; une table présente dans le schéma mais inconnue de la config **et** contenant une colonne ≥ 0,8 ; `on_unclassified: keep` sans le flag explicite.

### 5.6 Mode sûr par défaut

- `on_unclassified: fail` par défaut. Le run s'arrête **avant** d'écrire la première ligne, avec la liste des colonnes en cause.
- Aucun DSN, aucune clé, aucun mot de passe dans `dbclone.yaml` : uniquement via variables d'environnement ou fichier `.env.local` non versionné.
- En mode `anonymize`, la clé HMAC est générée aléatoirement au lancement, jamais écrite nulle part, jamais affichée.
- En mode `pseudonymize`, la clé vient de `$DBCLONE_KEY` ; l'outil avertit que la sortie reste une donnée personnelle.
- Refus de se connecter à une source dont `@@read_only = 0` sur MySQL (i.e. probablement un primaire) sans `--allow-primary`.
- Les tables `skip_tables` sortent avec leur structure mais zéro ligne.

### 5.7 Rapport

Affiché en fin d'exécution et écrit dans `.dbclone/last-report.json` :

```
dbclone transform — terminé en 4m12s
  Source        : mysqldump (32.1 GB, 61 244 019 lignes, 84 tables)
  Sortie        : dump_clean.sql.zst (2.9 GB)
  Mode          : anonymize (clé jetée)
  Colonnes      : 41 transformées, 3 mises à NULL, 2 gardées explicitement
  Tables vides  : audit_log, messenger_messages, sessions
  Troncatures   : 12 valeurs raccourcies (user.email VARCHAR(50))
  Non classées  : 0   ✔
  Débit         : 127 MB/s, 242 000 lignes/s, mémoire max 48 MB
```

### 5.8 Sous-échantillonnage cohérent (P1)

- On choisit une table racine (`user`) et une fraction.
- Sélection déterministe : `HMAC(clé, id) mod 10000 < fraction * 10000` → le même 5 % à chaque run en mode pseudonymize.
- Fermeture transitive via les clés étrangères (déclarées dans le schéma, ou déduites de Doctrine) : on garde toutes les lignes qui pointent vers un utilisateur gardé, et récursivement.
- Tables sans lien avec la racine (référentiels : pays, catégories, produits) : gardées intégralement.
- Contrainte : un dump SQL est séquentiel et les tables enfants peuvent précéder les parents. Stratégie : deux passes (première passe : collecte des identifiants gardés par table, stockés sur disque dans un index compact ; deuxième passe : filtrage). Le coût est de relire le dump, acceptable ; l'alternative "connexion directe" évite ce problème en interrogeant dans le bon ordre.

### 5.9 Performance et contraintes prod

| Exigence | Cible |
|---|---|
| Mémoire | Constante, < 100 Mo quel que soit le volume (hors index du sampling, qui va sur disque) |
| Débit `transform` | ≥ 100 Mo/s sur une machine moyenne (le parseur ne doit pas être le goulot ; `mysqldump` et le disque le seront) |
| Impact sur la source | Zéro requête émise par `transform` en mode dump. En mode connexion : `--single-transaction`, curseur côté serveur, `--throttle` |
| Charge système | `--nice`, `--ionice` appliqués par défaut au processus dump lancé via `pull` |
| Espace disque | Vérification avant démarrage ; sortie compressée en flux, jamais de dump brut intermédiaire |
| Sources autorisées | Réplica ou backup par défaut ; primaire refusé sans flag |

### 5.10 Intégrations (P2+)

- Bundle Symfony `dbclone/symfony-bundle` : commande `bin/console dbclone:init` qui appelle le binaire avec le bon `--doctrine` et le DSN de `.env`.
- Action GitHub / job GitLab CI : `dbclone check` sur chaque PR, `dbclone pull` pour les environnements de review.
- Docker image minimale (scratch + binaire).

---

## 6. Architecture

### 6.1 Organisation du workspace Cargo

```
dbclone/
├── Cargo.toml                  # workspace
├── crates/
│   ├── dbclone-cli/            # binaire : clap, orchestration des commandes, affichage
│   ├── dbclone-core/           # types partagés : Schema, Table, Column, ColumnType, Value, Report, erreurs
│   ├── dbclone-config/         # parsing/validation de dbclone.yaml, résolution des presets
│   ├── dbclone-dump-mysql/     # parseur streaming mysqldump → événements ; sérialiseur événements → SQL
│   ├── dbclone-dump-pg/        # idem pour pg_dump plain (P1)
│   ├── dbclone-transform/      # moteur : HMAC, registre des presets, application ligne par ligne
│   ├── dbclone-presets/        # implémentation des presets + données locales (listes de prénoms, villes…)
│   ├── dbclone-scan/           # heuristiques de détection (nom, contenu), scoring
│   ├── dbclone-doctrine/       # extraction du mapping depuis src/Entity (tokenizer PHP minimal)
│   ├── dbclone-sample/         # sous-échantillonnage par FK (P1)
│   └── dbclone-io/             # compression zstd/gzip, chiffrement, SSH, écriture atomique
└── fixtures/                   # dumps de test, configs, sorties attendues (golden files)
```

Chaque crate est testable seule. `dbclone-transform` ne connaît ni MySQL ni Postgres : il consomme un flux d'événements abstraits.

### 6.2 Le pipeline en flux

```
 stdin / fichier / ssh
        │
        ▼
 ┌──────────────┐   Event::TableSchema(Table)
 │  DumpParser  │   Event::RowsBegin(table)
 │  (mysql/pg)  │──▶Event::Row(Vec<Value>)          ──▶ ┌──────────────┐
 └──────────────┘   Event::RowsEnd                       │  Transformer │
                    Event::Raw(bytes)  (tout le reste,   │  (par table :│
                                       recopié tel quel) │   plan pré-  │
                                                          │   calculé)   │
                                                          └──────┬───────┘
                                                                 │ Event
                                                                 ▼
                                                          ┌──────────────┐    ┌──────────┐
                                                          │  Serializer  │──▶ │ zstd → fs│
                                                          └──────────────┘    └──────────┘
```

Points clés :
- **Le parseur ne comprend que ce dont il a besoin** : `CREATE TABLE` (pour les noms/types de colonnes) et `INSERT INTO ... VALUES`. Tout le reste (`SET`, `LOCK TABLES`, commentaires, triggers, vues) est transmis comme `Raw` sans interprétation. Robustesse maximale, surface minimale.
- **Plan de transformation par table** : à la réception de `TableSchema`, on résout une fois pour toutes `Vec<Option<Preset>>` indexé par position de colonne. Le chemin chaud (`Row`) ne fait aucune recherche par nom.
- **Zéro allocation inutile sur le chemin chaud** : les valeurs non transformées sont des slices vers le buffer d'entrée (`Cow<[u8]>`), seules les valeurs transformées sont allouées.
- **Parallélisme** : le parseur est séquentiel (il faut suivre le flux), mais la transformation des lignes d'un même `INSERT` peut être distribuée sur un pool (rayon) tout en préservant l'ordre de sortie. À mesurer : probablement inutile en v0.1 car le disque/`mysqldump` sont le goulot.

### 6.3 Parseur `mysqldump` : détails qui font mal

- Les `INSERT` étendus font des lignes de plusieurs Mo : lire par blocs, pas par lignes.
- Échappement MySQL : `\'`, `\"`, `\\`, `\n`, `\r`, `\0`, `\Z`, `''` dans certaines configs. Il faut un tokenizer d'état (hors chaîne / dans chaîne simple / dans chaîne double / échappement).
- Valeurs : `NULL`, nombres, chaînes, `0x...` (hex pour les BLOB), `_binary '...'`, dates, `b'...'`.
- `CREATE TABLE` : extraire nom de colonne, type, `NOT NULL`, longueur (`VARCHAR(255)`), `CHARACTER SET`. Ne pas essayer de parser les index/contraintes sauf `FOREIGN KEY` (pour le sampling).
- Noms de tables/colonnes entre backticks, éventuellement avec schéma préfixé.
- `--skip-extended-insert` produit un `INSERT` par ligne : même chemin de code.
- Tolérance : sur une ligne non comprise dans une table sensible → **erreur bloquante** (on ne laisse jamais passer une ligne non transformée d'une table sensible). Sur une table sans règle → transmission `Raw`.

### 6.4 Moteur de transformation

```rust
pub trait Preset: Send + Sync {
    /// Retourne la valeur transformée. `seed` = HMAC-SHA256(key, table.column?, value)
    /// selon la stratégie de cohérence (voir 6.5).
    fn apply(&self, input: &Value, seed: &[u8; 32], ctx: &ColumnCtx) -> Value;
    fn preserves_format(&self) -> bool;
    fn name(&self) -> &'static str;
}
```

- Le `seed` est un générateur déterministe : on en dérive un `ChaCha8Rng` pour piocher dans les listes (prénoms, villes) ou générer des chiffres.
- `ColumnCtx` : type SQL, longueur max, nullable, locale.

### 6.5 Stratégie de cohérence

Par défaut, le HMAC est calculé sur `preset_name || value` et **pas** sur le nom de table/colonne. Conséquence : `user.email = x` et `order.customer_email = x` donnent la même sortie. C'est le comportement attendu dans 95 % des cas.

Option `scope: column` sur un preset pour inclure `table.column` dans le HMAC quand on veut au contraire décorréler.

### 6.6 Dépendances envisagées

| Besoin | Crate |
|---|---|
| CLI | `clap` (derive) |
| Config | `serde`, `serde_yaml` (ou `serde_yml`) |
| Hash | `hmac`, `sha2` |
| RNG déterministe | `rand_chacha` |
| Compression | `zstd`, `flate2` |
| Async / IO | `std` d'abord ; `tokio` seulement pour le mode connexion directe (`sqlx` ou `mysql_async`) |
| Parallélisme | `rayon` (si mesuré utile) |
| Regex de scan | `regex` |
| Affichage | `indicatif` (barre de progression), `console` |
| Erreurs | `thiserror` (libs), `anyhow` (cli) |
| Tests | `insta` (snapshots/golden), `proptest` (parseur), `criterion` (bench) |
| Faker | **Pas** de crate faker générique : listes locales embarquées (`include_str!`) pour contrôler la qualité fr_FR et le déterminisme |

---

## 7. Sécurité et RGPD

- **Principe de minimisation** : l'outil ne lit que ce qu'il transforme ; le mode dump ne se connecte jamais à la base.
- **Aucun secret dans les fichiers versionnés** ; refus de démarrer si `dbclone.yaml` contient une clé `dsn`, `password` ou `key`.
- **Clé jetable** en mode anonymize ; **avertissement explicite** en mode pseudonymize.
- **Rapport signé** (P2) : hash du rapport + du fichier de config, pour prouver quelle config a produit quel dump.
- **Les colonnes texte libre** (`notes`, `comment`, `description`) sont toujours en `review` : c'est là que se cachent les PII imprévues ("appeler M. Dupont au 06…").
- **Champs JSON** : en `review` jusqu'à ce que le preset `json` existe.
- **Logs** : l'outil ne loggue jamais une valeur d'entrée, même en `--verbose`. Les messages d'erreur citent `table.colonne` et un numéro de ligne, jamais le contenu.
- Documentation à écrire : "ce que dbclone garantit / ne garantit pas" pour le DPO (pas de k-anonymat, pas de protection contre la ré-identification par croisement de données non sensibles comme `amount + date + postcode`).

---

## 8. Stratégie de tests

### 8.1 Niveaux

| Niveau | Outil | Ce qu'on vérifie |
|---|---|---|
| Unitaire presets | `cargo test` | Déterminisme (même seed → même sortie), format (IBAN mod 97, email regex, longueur ≤ colonne), NULL → NULL |
| Unitaire parseur | `proptest` | Round-trip : parser puis sérialiser un dump sans transformation = identique byte à byte |
| Golden | `insta` | `fixtures/*.sql` + `fixtures/*.yaml` → sortie snapshotée avec clé fixe `DBCLONE_KEY=test` |
| Intégration | Docker MySQL/Postgres en CI | Dump réel → transform → import → requêtes de vérification (`SELECT COUNT(*) WHERE email LIKE '%@gmail.com'` = 0) |
| Performance | `criterion` + dump synthétique 1 Go | Débit ≥ 100 Mo/s, mémoire plate |
| Sécurité | test dédié | `on_unclassified: fail` bloque bien ; aucune valeur d'entrée dans les logs |

### 8.2 Micro-tests fournis (dossier `examples/`)

Voir `examples/tests/README.md`. Chaque micro-test = un dump minuscule + une config + une liste d'assertions lisibles. Ils servent de spec exécutable avant même d'écrire le code.

---

## 9. Roadmap et jalons

### v0.1 — "transform" (4 à 6 week-ends)
- [ ] Workspace, CI, `cargo fmt`/`clippy` stricts
- [ ] Parseur streaming `mysqldump` + sérialiseur, round-trip byte à byte
- [ ] Config YAML + validation + résolution des presets
- [ ] Presets P0 (email, first/last/full_name, phone, null, constant, keep, hash, iban, address_line, city, postcode, date_shift)
- [ ] Listes fr_FR embarquées
- [ ] `transform` avec sortie fichier/stdout/zstd + rapport
- [ ] `init` avec signaux nom + contenu (sans Doctrine)
- [ ] `check` pour la CI
- [ ] `presets`
- [ ] Golden tests sur les fixtures fournies
- [ ] README avec le chrono des 10 minutes

**Critère de sortie** : le micro-test `examples/tests/01_basic` passe, et un vrai dump de 5 Go se transforme en < 2 min avec mémoire < 100 Mo.

### v0.2 — "pull" (4 à 6 week-ends)
- [ ] `--doctrine src/Entity` dans `init`
- [ ] Sampling par FK (deux passes)
- [ ] `restore` vers base locale
- [ ] `pull` via SSH, `--throttle`, `--nice`
- [ ] Refus du primaire sans flag
- [ ] Presets P1

### v0.3 — Postgres et équipe
- [ ] `pg_dump` plain, connexion directe
- [ ] `diff-schema`
- [ ] Chiffrement
- [ ] Bundle Symfony, GitHub Action, image Docker

### Plus tard
- Preset `json`, transformateurs WASM
- Version hébergée : snapshots planifiés, partage d'équipe, rapport de conformité signé

---

## 10. Risques et parades

| Risque | Impact | Parade |
|---|---|---|
| Le parseur `mysqldump` a des cas non prévus (charset exotique, `GENERATED` columns, partitions) | Corruption silencieuse du dump de sortie | Round-trip byte à byte en test ; tout ce qui n'est pas compris passe en `Raw` ; erreur bloquante sur une table sensible non comprise |
| Une PII passe à travers (colonne mal nommée, texte libre) | Fuite | `review` obligatoire sur texte libre ; signal contenu ; `check` en CI ; documentation honnête |
| Faux positifs du scan trop nombreux → config longue → chrono raté | Adoption | Mesurer sur 3 vrais projets Symfony open source (Sylius, EasyAdmin demo, Bolt) avant la v0.1 |
| Sampling casse l'intégrité référentielle sur les bases sans FK déclarées | Base locale inutilisable | Déduction des relations via Doctrine ; option `sampling.relations` manuelle |
| Volume : 32 Go = 45 min de dump côté serveur | Frustration | C'est `mysqldump` qui est lent, pas nous ; documenter ; proposer le mode connexion directe avec curseur + sampling à la source (v0.3) |
| Concurrence : quelqu'un reprend Replibyte ou Greenmask ajoute MySQL | Positionnement | Notre angle = Symfony/Doctrine + 10 minutes + MySQL first ; avancer vite sur v0.1 |

---

## 11. Questions ouvertes

1. Nom définitif et disponibilité (crates.io, GitHub, domaine).
2. Faut-il un mode `--dry-run` qui produit uniquement le rapport sans écrire ? (Probablement oui, P0, coût faible.)
3. `date_shift` : décalage global constant (préserve les intervalles entre dates d'un même client) ou par valeur ? Proposition : global par run en anonymize, dérivé de la clé en pseudonymize.
4. Que faire des colonnes `ENUM` et des `SET` MySQL ? (Toujours `keep` : pas de PII dedans par construction.)
5. Comment traiter les vues et procédures stockées qui pourraient recalculer une PII ? (Transmis `Raw` en v0.1, avertissement dans le rapport.)
6. Politique de longueur : quand la sortie du preset dépasse `VARCHAR(n)`, tronquer (défaut) ou échouer ?

---

## 12. Premier pas concret

```bash
cargo new --lib dbclone && cd dbclone
# 1. Copier examples/dumps/01_basic.sql dans fixtures/
# 2. Écrire le test : parse(01_basic.sql) |> serialize == 01_basic.sql  (round-trip)
# 3. Faire passer ce test. C'est la fondation de tout le reste.
```
