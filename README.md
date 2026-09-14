# Micro-tests

Chaque test = un dump + une config + des assertions. Ils sont écrits **avant** le code : c'est la spec exécutable. À porter en tests Rust (`insta` pour les golden, `assert_cmd` pour la CLI) dès que le binaire existe. Le script `run.sh` donne une version bash provisoire.

Variables communes : `DBCLONE_KEY=test-key-do-not-use-in-prod` quand le mode est `pseudonymize`.

---

## T01 — Round-trip sans transformation (fondation)

**Entrée** : `dumps/01_basic.sql`, puis `dumps/02_parser_edge_cases.sql`
**Commande** : `dbclone transform --passthrough < dump.sql > out.sql`
**Assertions** :
- `cmp dump.sql out.sql` → identique byte à byte (y compris commentaires, `/*!40101 ... */`, `DELIMITER`, hex, `_binary`, échappements, unicode).
- Mémoire max < 50 Mo (mesurer avec `/usr/bin/time -v`).

> Si ce test ne passe pas, rien d'autre n'a de sens. C'est le premier à écrire.

---

## T02 — Transformation de base

**Entrée** : `dumps/01_basic.sql` + `configs/01_basic.yaml`
**Commande** : `dbclone transform -c configs/01_basic.yaml < dumps/01_basic.sql > out.sql`
**Assertions sur `out.sql`** :

| # | Vérification | Comment |
|---|---|---|
| 1 | Aucune vraie valeur ne subsiste | `grep -c` de chacune des valeurs sensibles d'origine = 0 : `jean.dupont@gmail.com`, `O'Connor`, `0612345678`, `+33 7 98 76 54 32`, `1985-03-14`, `tok_a1b2c3d4e5f6`, `12 rue de la Paix`, `FR7630006000011234567890189`, `GB29NWBK60161331926819`, `AGRIFRPP`, `82.64.12.201`, `appeler M. Dupont` |
| 2 | Le nombre de lignes par table est conservé | `user` : 5, `address` : 3, `order` : 4, `bank_account` : 2, `product` : 2 |
| 3 | `audit_log` est vide | `CREATE TABLE \`audit_log\`` présent, aucun `INSERT INTO \`audit_log\`` |
| 4 | Les colonnes `keep` sont intactes | `149.90`, `ORD-2024-0001`, `'FR'`, `'GB'`, `Clavier mécanique`, `0x89504E470D0A1A0A` présents tels quels |
| 5 | Cohérence email inter-tables | l'email de `user.id=1` == `order.customer_email` des orders 1 et 2 (même valeur transformée) |
| 6 | Cohérence email intra-table | `user.id=1` et `user.id=5` (même email d'origine) ont le même email transformé |
| 7 | Format email préservé | toutes les valeurs `email` matchent `^[a-z0-9._-]+@example\.(org|net|com)$` |
| 8 | Format IBAN préservé | 2 IBAN, pays conservés (`FR`, `GB`), clé mod 97 valide, longueur correcte par pays (27 / 22) |
| 9 | Format téléphone fr préservé | `user.id=1,4` → 10 chiffres commençant par `06` ou `07` ; `user.id=2` (entrée E.164 `+33 …`) → sortie E.164 `+33 …` |
| 10 | NULL reste NULL | `user.id=3` : `phone` et `birth_date` sont `NULL` ; `address.id=2` : `line2` est `NULL` |
| 11 | `null` explicite | toutes les valeurs de `user.api_token`, `user.nickname`, `order.notes` sont `NULL` (`order.id=3` avait `''` → devient `NULL` car colonne nullable) |
| 12 | `constant` | les 5 `password` == `$2y$13$DEVONLY…` |
| 13 | `date_shift` | `birth_date` de `user.id=1` ≠ `1985-03-14`, mais dans `[1984-03-14, 1986-03-14]` |
| 14 | Longueur respectée | aucune valeur `email` > 180, `phone` > 20, `postcode` > 10 |
| 15 | `postcode keep_department` | `75002` → commence par `75` ; `69003` → `69` ; `SW1A 2AA` (GB) → format GB plausible, pas un code français |
| 16 | Le dump s'importe | `mysql < out.sql` sans erreur sur MySQL 8 (test d'intégration, CI Docker) |
| 17 | Rapport | stdout/`.dbclone/last-report.json` : `unclassified == 0`, `tables_skipped == ["audit_log"]`, `rows_processed == 16` |

---

## T03 — Refus si config incomplète (sûr par défaut)

**Entrée** : `dumps/01_basic.sql` + `configs/02_incomplete.yaml`
**Commandes** :
- `dbclone check -c configs/02_incomplete.yaml --from dumps/01_basic.sql`
- `dbclone transform -c configs/02_incomplete.yaml < dumps/01_basic.sql > out.sql`

**Assertions** :
- Code retour ≠ 0 pour les deux.
- `out.sql` est **vide** (0 octet). L'outil ne doit pas avoir commencé à écrire.
- stderr liste exactement les colonnes manquantes : `user.phone`, `user.password`, `user.api_token`, `user.birth_date`, `address.line1`, `address.line2`, `address.city`, `address.postcode`, `order.customer_email`, `bank_account.iban`, `bank_account.bic`, `bank_account.holder_name`, `audit_log.ip_address`, et les entrées `review` : `user.nickname`, `order.notes`, `audit_log.payload`, `product.name`.
- stderr ne contient **aucune valeur de données** (grep `gmail`, `Dupont`, `FR76` = 0).

---

## T04 — Déterminisme en mode pseudonymize

**Entrée** : `dumps/01_basic.sql` + `configs/03_pseudonymize.yaml`
**Commandes** :
```
DBCLONE_KEY=test-key-do-not-use-in-prod dbclone transform -c configs/03_pseudonymize.yaml < dumps/01_basic.sql > run1.sql
DBCLONE_KEY=test-key-do-not-use-in-prod dbclone transform -c configs/03_pseudonymize.yaml < dumps/01_basic.sql > run2.sql
DBCLONE_KEY=another-key                 dbclone transform -c configs/03_pseudonymize.yaml < dumps/01_basic.sql > run3.sql
```
**Assertions** :
- `cmp run1.sql run2.sql` → identique.
- `cmp run1.sql run3.sql` → différent (au moins les emails).
- Sans `DBCLONE_KEY` → code retour ≠ 0, message clair.
- Un avertissement "sortie pseudonymisée = donnée personnelle" est affiché sur stderr.
- `run1.sql` est le golden file de référence à snapshotter avec `insta`.

## T04b — Non-déterminisme en mode anonymize

Deux exécutions de T02 produisent des sorties **différentes** (clé aléatoire), mais chacune passe toutes les assertions de T02.

---

## T05 — Cas limites du parseur avec transformation

**Entrée** : `dumps/02_parser_edge_cases.sql` + `configs/02_parser_edge_cases.yaml`
**Assertions** :
- Seules les valeurs des colonnes `email` diffèrent entre entrée et sortie. Test : remplacer par un placeholder toutes les valeurs de position `email` dans les deux fichiers, puis `cmp`.
- La chaîne `'INSERT INTO \`user\` VALUES (1,''x''); -- pas une vraie requête'` ressort intacte (pas interprétée comme une requête).
- `email_domain` (GENERATED) n'est ni exigée par `check` ni "transformée".
- La vue `v_emails` et le trigger ressortent tels quels ; le rapport contient un avertissement `raw_objects: ["VIEW v_emails", "TRIGGER trg_order"]`.
- `empty_table` : 1 ligne, l'email unicode est transformé, le reste de la ligne intact.
- Hex `0xDEADBEEF` et `_binary '\0…'` intacts.

---

## T06 — Génération de config par `init`

**Entrée** : `dumps/01_basic.sql`
**Commande** : `dbclone init --from dumps/01_basic.sql -o generated.yaml`
**Assertions** :
- `generated.yaml` est sémantiquement égal à `configs/01_basic.expected-init.yaml` (comparer les structures YAML, pas le texte : les commentaires et l'ordre peuvent différer).
- Chaque entrée porte un commentaire indiquant le signal (`# nom`, `# nom + contenu`, …).
- `dbclone check -c generated.yaml --from dumps/01_basic.sql` échoue (section `review` non vide) : c'est voulu, l'humain doit trancher.

## T06b — `init` avec Doctrine

**Commande** : `dbclone init --from dumps/01_basic.sql --doctrine doctrine/ -o generated.yaml`
**Assertions supplémentaires** :
- `bank_account.iban: iban` et `bank_account.bic: bic` détectés via `Assert\Iban` / `Assert\Bic` (et non via le nom de propriété `accountNumber` / `swift`).
- `user.email` porte le commentaire `# Assert\Email`.
- `user.password: constant(...)` justifié par `PasswordAuthenticatedUserInterface`.
- Le rapport d'init liste les relations déduites : `address.user_id → user.id`, `bank_account.user_id → user.id`.

---

## T07 — Sous-échantillonnage (v0.2)

**Entrée** : `dumps/01_basic.sql` + `configs/03_pseudonymize.yaml` + option `--sample user:60%`
**Assertions** :
- Le nombre de `user` gardés est déterministe pour une clé donnée (avec 5 lignes et 60 %, on attend 3 ± 1 ; le golden fixe la valeur exacte).
- Pour chaque `user` gardé, **toutes** ses `address`, `order`, `bank_account` sont présentes ; pour chaque `user` supprimé, **aucune**.
- `product` (référentiel, sans FK vers user) est intégralement conservé.
- `mysql < out.sql` avec `FOREIGN_KEY_CHECKS=1` réussit.

---

## T08 — Performance (bench, pas un test bloquant)

Générer un dump synthétique de 1 Go (script `gen_big_dump.sh` à écrire : 1 table `user` × 5 M lignes) :
- `transform` avec `01_basic.yaml` adapté : débit ≥ 100 Mo/s, mémoire max < 100 Mo, sur un laptop récent.
- Comparer à un script Python `re.sub` équivalent pour le README (ordre de grandeur attendu : 20 à 50×).
