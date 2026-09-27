# Changelog

Toutes les évolutions notables de ce projet sont documentées dans ce fichier.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/), et le projet respecte le [versionnage sémantique](https://semver.org/lang/fr/).

## [0.1.0] - à venir

Première version publique.

### Ajouté

- Commande `transform` : anonymise ou pseudonymise un dump `mysqldump` en streaming, depuis un fichier ou stdin, vers un fichier ou stdout, avec une mémoire constante quelle que soit la taille du dump.
- Commande `init` : analyse un dump (noms de colonnes et échantillon de contenu) et génère un `sosie.yaml` de départ, avec une section `review` pour les colonnes ambiguës.
- Commande `check` : vérifie que la config couvre toutes les colonnes sensibles d'un dump.
- Commande `presets` : liste les presets disponibles avec un exemple généré.
- 13 presets déterministes (locale `fr_FR`) : `hash`, `email`, `first_name`, `last_name`, `full_name`, `phone`, `date_shift`, `iban`, `bic`, `address_line`, `city`, `postcode`, `ip`.
- Deux modes : `anonymize` (clé aléatoire à chaque exécution) et `pseudonymize` (clé lue dans `SOSIE_KEY`, sortie stable d'une exécution à l'autre).
- Règles par colonne : `keep`, `null`, `constant("…")` ou preset avec paramètres ; tables exclues via `skip_tables` / `truncate_tables` (structure conservée, sans lignes).
- Garde-fous de sécurité : refus de tourner si une colonne sensible n'a pas de règle, et refus de toute clé secrète (`dsn`, `password`, `key`) dans le fichier de config.
- Respect du schéma : unicité garantie des valeurs générées sur les colonnes `PRIMARY KEY` / `UNIQUE`, chaîne vide au lieu de `NULL` sur les colonnes `NOT NULL`, colonnes `GENERATED` ignorées.
- Réécriture à l'identique de tout ce qui n'est pas transformé (round-trip octet pour octet vérifié).
- Rapport de fin d'exécution (compteurs uniquement, jamais une valeur de donnée), avec détail par colonne via `--verbose` et rapport JSON détaillé.
- Barre de progression sur stderr (pourcentage, débit, ETA, table courante), masquée hors terminal.
- Option `--dry-run` : valide la config et le dump sans écrire la sortie.
- Exemple `gen_dump` : générateur de dump synthétique réaliste pour tester à grande échelle.
- Binaires précompilés pour Linux, macOS (Intel et Apple Silicon) et Windows, publiés sur les Releases GitHub.

### Limitations connues

- MySQL uniquement, locale `fr_FR` uniquement.
- Pas de sortie compressée : à compresser soi-même (`sosie transform … | zstd -o out.sql.zst`).

[0.1.0]: https://github.com/mlejeunedev/sosie/releases/tag/v0.1.0
