# Sosie — configuration : valeurs de règle et modes

> Détails techniques sur le contenu de `sosie.yaml`. Pour la liste des commandes et le cas d'usage complet, voir `docs/USAGE.md` ; pour l'installation et le démarrage rapide, voir le `README.md`.

## Les 4 valeurs possibles pour une règle de colonne

C'est toute la grammaire acceptée sous `tables.<table>.<colonne>` — il n'y en a pas d'autre.

| Forme | Exemple | Effet |
|---|---|---|
| `keep` | `id: keep` | copie telle quelle, explicitement (fait taire le scan) |
| `null` | `api_token: null` | vide (`NULL`, ou chaîne vide si la colonne est `NOT NULL`) |
| `constant("...")` | `password: constant("x")` | toujours la même valeur fixe |
| un nom de preset, nu ou en map | `email: email` ou `{ preset: date_shift, days: 365 }` | valeur fictive plausible, déterministe (voir le tableau des presets dans `docs/USAGE.md`) |

Deux pièges à connaître :

- **`review` n'est pas une valeur de règle.** La section `review:` générée par `init` est juste une liste informative (`table.colonne  # raison`) ; écrire `nickname: review` serait interprété comme un nom de preset inconnu et rejeté. Pour résoudre un item de `review`, choisis une des 4 vraies formes ci-dessus.
- **Tous les presets du cahier des charges ne sont pas implémentés.** Seuls les 13 listés dans `docs/USAGE.md` existent en v0.1. Si une colonne aurait besoin d'un preset non implémenté (`siret`, `company`, `lorem`, `regex_mask`, `json`...), tu es limité à `keep`/`null`/`constant` en attendant.

### Au niveau de la table entière

En plus des règles par colonne, deux leviers s'appliquent à une table entière :

```yaml
skip_tables:
  - audit_log        # structure gardée, zéro ligne dans la sortie
truncate_tables:
  - session          # idem : pratique pour une table entièrement à risque
```

Pratique quand une table entière est à risque plutôt que de traiter colonne par colonne — les colonnes de ces tables n'ont pas besoin de règle, `check` les exempte automatiquement.

## `anonymize` ou `pseudonymize` ?

```yaml
mode: anonymize        # ou pseudonymize
```

Les deux modes appliquent les mêmes presets ; ce qui change, c'est l'origine et la durée de vie de la clé HMAC qui pilote le tirage déterministe des valeurs fictives.

### `anonymize` (le défaut)

```bash
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

- La clé est générée aléatoirement **à chaque exécution**, utilisée en mémoire, puis jetée. Elle n'est jamais affichée, jamais écrite sur disque.
- Deux exécutions du même dump donnent donc des valeurs fictives **différentes** à chaque fois.
- Irréversible même en théorie : personne — pas même toi — ne peut retrouver la valeur d'origine à partir de la sortie.
- C'est le mode à utiliser par défaut, notamment pour tout ce qui sort de ton contrôle (partage, CI, environnement de démo).

### `pseudonymize`

```bash
export SOSIE_KEY="une-phrase-secrete-d-au-moins-16-caracteres"
sosie transform --from dump.sql --config sosie.yaml --out dump_clean.sql
```

- La clé vient de la variable d'environnement `SOSIE_KEY` (minimum 16 caractères), jamais du fichier de config (refusé au chargement si tu essaies).
- **Déterministe** : même dump + même clé + même preset ⇒ toujours les mêmes valeurs fictives. `jean.dupont@gmail.com` donnera par exemple systématiquement le même faux email, y compris dans un export fait un mois plus tard.
- Utile quand tu dois recouper plusieurs dumps dans le temps (comparer deux exports, garder la cohérence entre plusieurs bases de dev/staging), ou garder `user.id=42` reconnaissable à travers des tables sans clé étrangère déclarée.
- Sans `SOSIE_KEY` défini, ou trop courte, `check`/`transform` refusent de démarrer avec un message clair.

**Attention** : une sortie `pseudonymize` reste juridiquement une donnée personnelle (RGPD) — quelqu'un qui connaît `$SOSIE_KEY` peut vérifier ou retrouver une valeur d'origine (et même sans la clé, une attaque par dictionnaire sur des emails courants reste possible, HMAC étant déterministe). Traite le dump résultant avec les mêmes précautions que la prod : ne le partage pas plus largement qu'un dump `anonymize`, et garde `SOSIE_KEY` hors du dépôt (variable d'environnement, coffre-fort de secrets — jamais en clair dans un fichier versionné).

### Lequel choisir ?

| Besoin | Mode |
|---|---|
| Copie ponctuelle pour debug local | `anonymize` |
| Partager un dump avec quelqu'un d'externe | `anonymize` |
| Comparer deux exports pris à des dates différentes | `pseudonymize` |
| Garder la cohérence entre plusieurs bases (dev, staging, CI) sans clés étrangères déclarées | `pseudonymize` |
