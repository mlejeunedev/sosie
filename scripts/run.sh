#!/usr/bin/env bash
# Runner provisoire des micro-tests (T01 → T05). À remplacer par `cargo test`
# dès que le binaire existe. Usage : ./run.sh [chemin/vers/dbclone]
set -euo pipefail

BIN="${1:-dbclone}"
HERE="$(cd "$(dirname "$0")" && pwd)"
EX="$HERE/.."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pass=0; fail=0
ok()   { echo "  ✔ $1"; pass=$((pass+1)); }
ko()   { echo "  ✘ $1"; fail=$((fail+1)); }
check(){ if eval "$2"; then ok "$1"; else ko "$1"; fi; }

echo "T01 — round-trip"
for d in 01_basic 02_parser_edge_cases; do
  "$BIN" transform --passthrough < "$EX/dumps/$d.sql" > "$TMP/$d.rt.sql"
  check "$d identique byte à byte" "cmp -s '$EX/dumps/$d.sql' '$TMP/$d.rt.sql'"
done

echo "T02 — transformation de base"
"$BIN" transform -c "$EX/configs/01_basic.yaml" < "$EX/dumps/01_basic.sql" > "$TMP/out.sql"
for v in 'jean.dupont@gmail.com' "O'Connor" '0612345678' '+33 7 98 76 54 32' '1985-03-14' \
         'tok_a1b2c3d4e5f6' '12 rue de la Paix' 'FR7630006000011234567890189' \
         'GB29NWBK60161331926819' 'AGRIFRPP' '82.64.12.201' 'appeler M. Dupont'; do
  check "valeur absente : $v" "! grep -qF -- \"$v\" '$TMP/out.sql'"
done
check "audit_log vidée"        "! grep -q 'INSERT INTO \`audit_log\`' '$TMP/out.sql'"
check "audit_log structure OK" "grep -q 'CREATE TABLE \`audit_log\`' '$TMP/out.sql'"
check "keep amount"            "grep -qF '149.90' '$TMP/out.sql'"
check "keep blob hex"          "grep -qF '0x89504E470D0A1A0A' '$TMP/out.sql'"
check "constant password"      "[ \$(grep -o '\$2y\$13\$DEVONLY' '$TMP/out.sql' | wc -l) -eq 5 ]"
check "emails au bon format"   "! grep -oE \"'[^']*@[^']*'\" '$TMP/out.sql' | grep -vE \"@example\\.(org|net|com)'\$\" | grep -q ."

echo "T03 — refus si config incomplète"
: > "$TMP/empty.sql"
set +e
"$BIN" check -c "$EX/configs/02_incomplete.yaml" --from "$EX/dumps/01_basic.sql" >/dev/null 2>"$TMP/err.txt"; rc1=$?
"$BIN" transform -c "$EX/configs/02_incomplete.yaml" < "$EX/dumps/01_basic.sql" > "$TMP/empty.sql" 2>>"$TMP/err.txt"; rc2=$?
set -e
check "check échoue"                "[ $rc1 -ne 0 ]"
check "transform échoue"            "[ $rc2 -ne 0 ]"
check "aucun octet écrit"           "[ ! -s '$TMP/empty.sql' ]"
check "stderr cite user.phone"      "grep -q 'user.phone' '$TMP/err.txt'"
check "stderr sans donnée réelle"   "! grep -qiE 'gmail|dupont|FR76' '$TMP/err.txt'"

echo "T04 — déterminisme pseudonymize"
export DBCLONE_KEY=test-key-do-not-use-in-prod
"$BIN" transform -c "$EX/configs/03_pseudonymize.yaml" < "$EX/dumps/01_basic.sql" > "$TMP/run1.sql"
"$BIN" transform -c "$EX/configs/03_pseudonymize.yaml" < "$EX/dumps/01_basic.sql" > "$TMP/run2.sql"
DBCLONE_KEY=another-key "$BIN" transform -c "$EX/configs/03_pseudonymize.yaml" < "$EX/dumps/01_basic.sql" > "$TMP/run3.sql"
check "run1 == run2"  "cmp -s '$TMP/run1.sql' '$TMP/run2.sql'"
check "run1 != run3"  "! cmp -s '$TMP/run1.sql' '$TMP/run3.sql'"
unset DBCLONE_KEY

echo "T05 — cas limites avec transformation"
"$BIN" transform -c "$EX/configs/02_parser_edge_cases.yaml" < "$EX/dumps/02_parser_edge_cases.sql" > "$TMP/edge.sql"
check "faux INSERT dans chaîne intact" "grep -qF \"INSERT INTO \\\`user\\\` VALUES (1,''x''); -- pas une vraie requête\" '$TMP/edge.sql'"
check "vue transmise"                  "grep -qF 'VIEW \`v_emails\`' '$TMP/edge.sql'"
check "hex intact"                     "grep -qF '0xDEADBEEF' '$TMP/edge.sql'"
check "m@m.fr transformé"              "! grep -qF 'm@m.fr' '$TMP/edge.sql'"

echo
echo "$pass réussis, $fail échoués"
[ "$fail" -eq 0 ]
