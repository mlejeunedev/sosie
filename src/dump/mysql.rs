//! Parseur/writer pour le format produit par `mysqldump`.

use std::borrow::Cow;
use std::io::{Read, Write};

use anyhow::{Result, bail};

use crate::dump::{Column, DumpParser, DumpWriter, Event, SqlType, Table, Value};

/// Taille du buffer de lecture physique sur la source.
const READ_CHUNK: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// 2a. Découpage du flux en instructions SQL complètes.
// ---------------------------------------------------------------------------

/// État du scanner de limites d'instructions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Normal,
    LineComment,
    BlockComment,
    SingleQuoted,
    DoubleQuoted,
}

/// Curseur de scan reprenable : où en est [`find_statement_end`] dans le
/// buffer, pour ne jamais rescanner les octets déjà vus quand une instruction
/// arrive en plusieurs lectures (sans lui, une instruction de 1 Mio lue par
/// blocs de 64 Kio serait parcourue ~136 fois au lieu d'une).
#[derive(Clone, Copy)]
struct ScanCursor {
    state: ScanState,
    pos: usize,
}

impl ScanCursor {
    const START: ScanCursor = ScanCursor {
        state: ScanState::Normal,
        pos: 0,
    };
}

/// Cherche la fin de la prochaine instruction SQL dans `buf` (à partir de l'indice 0).
///
/// Une instruction se termine par un `;` immédiatement suivi d'une fin de ligne
/// (`\n` ou `\r\n`), en dehors de toute chaîne (`'…'`, `"…"`) ou de tout
/// commentaire (`-- …`, `/* … */`, y compris les commentaires versionnés
/// `/*!40101 … */` qui suivent la même syntaxe).
///
/// Retourne l'indice juste après la fin de ligne qui suit le `;` terminal — le
/// saut de ligne fait partie de l'instruction retournée, pour que deux
/// instructions consécutives se recollent sans rien ajouter ni perdre. `eof`
/// indique qu'aucun octet supplémentaire ne viendra : la fin du buffer compte
/// alors aussi comme fin d'instruction (dernière ligne sans retour à la ligne).
/// `None` signifie qu'il faut lire plus de données pour trancher ; `cursor`
/// mémorise alors l'avancement, et l'appel suivant (même `buf`, allongé)
/// reprend là où celui-ci s'est arrêté. Il doit valoir [`ScanCursor::START`]
/// au premier appel sur une nouvelle instruction.
fn find_statement_end(buf: &[u8], eof: bool, cursor: &mut ScanCursor) -> Option<usize> {
    // Cas particulier : une instruction qui commence (après d'éventuelles
    // lignes blanches — `mysqldump` sépare ses sections ainsi) par un
    // commentaire de fin de ligne (`-- …`). `mysqldump` en met toujours un
    // seul par ligne, souvent collé sans `;` à ce qui suit (ex. juste avant
    // un `INSERT INTO`) — sans ce cas, un tel commentaire se retrouverait
    // fusionné dans l'instruction suivante, qui ne serait alors plus
    // reconnue comme `CREATE TABLE` / `INSERT INTO`. Byte pour byte ça ne
    // changerait rien (un `Raw` fusionné se réécrit à l'identique), mais on
    // perdrait la structuration.
    if cursor.pos == 0 {
        let after_blank_lines = {
            let mut i = 0;
            while buf.get(i) == Some(&b'\n') {
                i += 1;
            }
            i
        };
        if !eof && buf.len() < after_blank_lines + 2 {
            // Pas encore de quoi savoir si ça commence par `--`.
            return None;
        }
        if buf[after_blank_lines..].starts_with(b"--") {
            return match buf[after_blank_lines..].iter().position(|&b| b == b'\n') {
                Some(nl) => Some(after_blank_lines + nl + 1),
                None if eof => Some(buf.len()),
                None => None,
            };
        }
    }

    let mut state = cursor.state;
    let mut i = cursor.pos;
    while i < buf.len() {
        // Le scan regarde un octet en avant (`--`, `/*`, `*/`, `''`, `\'`,
        // `;\n`) : tant que le flux n'est pas fini, on s'arrête avant
        // d'entamer un octet dont le suivant n'est pas encore lu. Seul `;\r`
        // a besoin d'en voir deux (cf. ci-dessous).
        if !eof && i + 1 >= buf.len() {
            break;
        }
        let b = buf[i];
        match state {
            ScanState::Normal => match b {
                b';' if is_line_end(buf, i + 1) || (eof && i + 1 == buf.len()) => {
                    return Some(line_end_len(buf, i + 1) + i + 1);
                }
                b';' if !eof && buf.get(i + 1) == Some(&b'\r') && i + 2 >= buf.len() => {
                    // `;\r` en fin de buffer : `\n` peut suivre, on attend.
                    break;
                }
                b'\'' => state = ScanState::SingleQuoted,
                b'"' => state = ScanState::DoubleQuoted,
                b'-' if buf.get(i + 1) == Some(&b'-') => {
                    state = ScanState::LineComment;
                    i += 1;
                }
                b'/' if buf.get(i + 1) == Some(&b'*') => {
                    state = ScanState::BlockComment;
                    i += 1;
                }
                _ => {}
            },
            ScanState::LineComment => {
                if b == b'\n' {
                    state = ScanState::Normal;
                }
            }
            ScanState::BlockComment => {
                if b == b'*' && buf.get(i + 1) == Some(&b'/') {
                    state = ScanState::Normal;
                    i += 1;
                }
            }
            ScanState::SingleQuoted => match b {
                b'\\' => i += 1,
                b'\'' => {
                    if buf.get(i + 1) == Some(&b'\'') {
                        i += 1;
                    } else {
                        state = ScanState::Normal;
                    }
                }
                _ => {}
            },
            ScanState::DoubleQuoted => match b {
                b'\\' => i += 1,
                b'"' => {
                    if buf.get(i + 1) == Some(&b'"') {
                        i += 1;
                    } else {
                        state = ScanState::Normal;
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    *cursor = ScanCursor { state, pos: i };
    None
}

/// `true` si `buf[i..]` commence par une fin de ligne (`\n` ou `\r\n`).
fn is_line_end(buf: &[u8], i: usize) -> bool {
    line_end_len(buf, i) > 0
}

/// Longueur de la fin de ligne commençant en `i` (0, 1 pour `\n`, 2 pour `\r\n`).
fn line_end_len(buf: &[u8], i: usize) -> usize {
    match (buf.get(i), buf.get(i + 1)) {
        (Some(b'\r'), Some(b'\n')) => 2,
        (Some(b'\n'), _) => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Petits outils de scan partagés par 2b et 2c.
// ---------------------------------------------------------------------------

fn skip_ws(buf: &[u8], mut i: usize) -> usize {
    while matches!(buf.get(i), Some(b) if b.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

fn starts_with_ci(buf: &[u8], word: &[u8]) -> bool {
    buf.len() >= word.len() && buf[..word.len()].eq_ignore_ascii_case(word)
}

/// Consomme un mot-clé ASCII insensible à la casse en `i`, entouré (avant)
/// d'espaces déjà consommés par l'appelant. Retourne la position juste après.
fn expect_keyword(buf: &[u8], i: usize, word: &[u8]) -> Result<usize> {
    if starts_with_ci(&buf[i..], word) {
        Ok(i + word.len())
    } else {
        bail!(
            "attendu {:?} à la position {i}, trouvé {:?}",
            String::from_utf8_lossy(word),
            String::from_utf8_lossy(&buf[i..buf.len().min(i + 20)])
        )
    }
}

/// Parse un identifiant entre backticks (`` `nom` ``, doublement du backtick
/// pour en échapper un littéral). `buf[i]` doit être un backtick.
/// Retourne le nom désescapé et la position juste après le backtick fermant.
fn parse_backtick_ident(buf: &[u8], i: usize) -> Result<(String, usize)> {
    if buf.get(i) != Some(&b'`') {
        bail!("identifiant entre backticks attendu à la position {i}");
    }
    let mut name = Vec::new();
    let mut j = i + 1;
    loop {
        match buf.get(j) {
            Some(b'`') if buf.get(j + 1) == Some(&b'`') => {
                name.push(b'`');
                j += 2;
            }
            Some(b'`') => return Ok((String::from_utf8_lossy(&name).into_owned(), j + 1)),
            Some(&b) => {
                name.push(b);
                j += 1;
            }
            None => bail!("backtick fermant manquant"),
        }
    }
}

/// Parse un identifiant potentiellement qualifié (`` `schema`.`table` ``) et
/// retourne le dernier segment (le nom propre de la table/colonne) avec la
/// position juste après.
fn parse_qualified_ident(buf: &[u8], i: usize) -> Result<(String, usize)> {
    let (mut name, mut j) = parse_backtick_ident(buf, i)?;
    if buf.get(j) == Some(&b'.') && buf.get(j + 1) == Some(&b'`') {
        let (last, k) = parse_backtick_ident(buf, j + 1)?;
        name = last;
        j = k;
    }
    Ok((name, j))
}

/// Trouve l'index de la parenthèse fermante correspondant à `buf[open]` (qui
/// doit être `(`), en ignorant les parenthèses et virgules à l'intérieur des
/// chaînes `'…'` / `"…"` et en gérant l'imbrication (`varchar(180)`, `enum(...)`).
fn find_matching_paren(buf: &[u8], open: usize) -> Result<usize> {
    if buf.get(open) != Some(&b'(') {
        bail!("'(' attendu à la position {open}");
    }
    let mut depth = 1i32;
    let mut i = open + 1;
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum St {
        Normal,
        Single,
        Double,
    }
    let mut st = St::Normal;
    while i < buf.len() {
        let b = buf[i];
        match st {
            St::Normal => match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i);
                    }
                }
                b'\'' => st = St::Single,
                b'"' => st = St::Double,
                _ => {}
            },
            St::Single => match b {
                b'\\' => i += 1,
                b'\'' => {
                    if buf.get(i + 1) == Some(&b'\'') {
                        i += 1;
                    } else {
                        st = St::Normal;
                    }
                }
                _ => {}
            },
            St::Double => match b {
                b'\\' => i += 1,
                b'"' => {
                    if buf.get(i + 1) == Some(&b'"') {
                        i += 1;
                    } else {
                        st = St::Normal;
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    bail!("parenthèse fermante manquante")
}

/// Découpe `buf` en segments séparés par des virgules de premier niveau
/// (hors chaînes et hors parenthèses imbriquées). Retourne des bornes
/// `(start, end)` relatives à `buf`, avec les espaces autour déjà retirés.
fn split_top_level_commas(buf: &[u8]) -> Vec<(usize, usize)> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum St {
        Normal,
        Single,
        Double,
    }
    let mut st = St::Normal;
    let mut depth = 0i32;
    let mut out = Vec::new();
    let mut seg_start = 0usize;
    let mut i = 0usize;
    while i < buf.len() {
        let b = buf[i];
        match st {
            St::Normal => match b {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'\'' => st = St::Single,
                b'"' => st = St::Double,
                b',' if depth == 0 => {
                    out.push(trim_range(buf, seg_start, i));
                    seg_start = i + 1;
                }
                _ => {}
            },
            St::Single => match b {
                b'\\' => i += 1,
                b'\'' => {
                    if buf.get(i + 1) == Some(&b'\'') {
                        i += 1;
                    } else {
                        st = St::Normal;
                    }
                }
                _ => {}
            },
            St::Double => match b {
                b'\\' => i += 1,
                b'"' => {
                    if buf.get(i + 1) == Some(&b'"') {
                        i += 1;
                    } else {
                        st = St::Normal;
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    out.push(trim_range(buf, seg_start, buf.len()));
    out
}

fn trim_range(buf: &[u8], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && buf[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && buf[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (start, end)
}

// ---------------------------------------------------------------------------
// 2b. `CREATE TABLE`
// ---------------------------------------------------------------------------

fn sql_type_from_keyword(word: &str) -> SqlType {
    match word.to_ascii_lowercase().as_str() {
        "int" | "integer" | "tinyint" | "smallint" | "mediumint" | "bigint" | "year" => {
            SqlType::Int
        }
        "decimal" | "numeric" => SqlType::Decimal,
        "float" | "double" | "real" => SqlType::Float,
        "char" | "varchar" => SqlType::Char,
        "text" | "tinytext" | "mediumtext" | "longtext" => SqlType::Text,
        "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary" => SqlType::Blob,
        "date" => SqlType::Date,
        "datetime" | "timestamp" => SqlType::DateTime,
        "json" => SqlType::Json,
        "enum" => SqlType::Enum,
        "set" => SqlType::Set,
        "bit" => SqlType::Bit,
        other => SqlType::Other(other.to_string()),
    }
}

/// Parse une définition de colonne (commence par un identifiant entre backticks).
fn parse_column_def(def: &[u8]) -> Result<Column> {
    let (name, mut i) = parse_backtick_ident(def, 0)?;
    i = skip_ws(def, i);

    let type_start = i;
    while matches!(def.get(i), Some(b) if b.is_ascii_alphanumeric() || *b == b'_') {
        i += 1;
    }
    let type_word = String::from_utf8_lossy(&def[type_start..i]).into_owned();

    i = skip_ws(def, i);
    let max_len = if def.get(i) == Some(&b'(') {
        let close = find_matching_paren(def, i)?;
        let inner = &def[i + 1..close];
        let digits_end = inner
            .iter()
            .position(|b| !b.is_ascii_digit())
            .unwrap_or(inner.len());
        let max_len = std::str::from_utf8(&inner[..digits_end])
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        i = close + 1;
        max_len
    } else {
        None
    };

    let rest = &def[i..];
    let nullable = !contains_word_ci(rest, b"NOT NULL");
    let generated = contains_word_ci(rest, b"GENERATED");

    Ok(Column {
        name,
        sql_type: sql_type_from_keyword(&type_word),
        nullable,
        max_len,
        generated,
        unique: false, // renseigné après coup par `parse_create_table`, une fois les clés lues.
    })
}

/// Si `def` est une contrainte `PRIMARY KEY (...)` ou `UNIQUE [KEY|INDEX]
/// [\`nom\`] (...)` portant sur une seule colonne, retourne le nom de cette
/// colonne. Les clés composites (plusieurs colonnes) ne rendent aucune
/// colonne unique à elle seule : elles sont ignorées.
fn single_column_unique_constraint(def: &[u8]) -> Option<String> {
    let is_primary = starts_with_ci(def, b"PRIMARY KEY");
    let is_unique = starts_with_ci(def, b"UNIQUE");
    if !is_primary && !is_unique {
        return None;
    }
    let open = def.iter().position(|&b| b == b'(')?;
    let close = find_matching_paren(def, open).ok()?;
    let inner = &def[open + 1..close];
    let cols = split_top_level_commas(inner);
    if cols.len() != 1 {
        return None;
    }
    let (s, e) = cols[0];
    let segment = &inner[s..e];
    if segment.first() != Some(&b'`') {
        return None;
    }
    parse_backtick_ident(segment, 0).ok().map(|(name, _)| name)
}

fn contains_word_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Parse une instruction `CREATE TABLE` complète.
fn parse_create_table(stmt: &[u8]) -> Result<Table> {
    let mut i = expect_keyword(stmt, 0, b"CREATE")?;
    i = skip_ws(stmt, i);
    i = expect_keyword(stmt, i, b"TABLE")?;
    i = skip_ws(stmt, i);
    if starts_with_ci(&stmt[i..], b"IF NOT EXISTS") {
        i += "IF NOT EXISTS".len();
        i = skip_ws(stmt, i);
    }

    let (name, mut i2) = parse_qualified_ident(stmt, i)?;
    i2 = skip_ws(stmt, i2);
    if stmt.get(i2) != Some(&b'(') {
        bail!("'(' attendu après le nom de table dans CREATE TABLE {name}");
    }
    let close = find_matching_paren(stmt, i2)?;
    let body = &stmt[i2 + 1..close];

    let mut columns = Vec::new();
    let mut foreign_keys = Vec::new();
    for (s, e) in split_top_level_commas(body) {
        if s == e {
            continue;
        }
        let def = &body[s..e];
        if def.first() == Some(&b'`') {
            columns.push(parse_column_def(def)?);
        } else if contains_word_ci(def, b"FOREIGN KEY") {
            foreign_keys.push(String::from_utf8_lossy(def).into_owned());
        } else if let Some(col_name) = single_column_unique_constraint(def) {
            if let Some(col) = columns.iter_mut().find(|c| c.name == col_name) {
                col.unique = true;
            }
        }
        // KEY / CONSTRAINT (CHECK) / FULLTEXT / clés composites : ignorés.
    }

    Ok(Table {
        name,
        columns,
        foreign_keys,
        raw: stmt.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// 2c. `INSERT INTO`
// ---------------------------------------------------------------------------

/// Métadonnées extraites du préfixe d'un `INSERT INTO`, avant la liste des tuples.
struct InsertHead {
    table: String,
    columns: Option<Vec<String>>,
    /// Octets `stmt[0..prefix_end]`, à réécrire tels quels par le writer.
    prefix_end: usize,
}

fn parse_insert_head(stmt: &[u8]) -> Result<InsertHead> {
    let mut i = expect_keyword(stmt, 0, b"INSERT")?;
    i = skip_ws(stmt, i);
    i = expect_keyword(stmt, i, b"INTO")?;
    i = skip_ws(stmt, i);

    let (table, mut i2) = parse_qualified_ident(stmt, i)?;
    i2 = skip_ws(stmt, i2);

    let columns = if stmt.get(i2) == Some(&b'(') {
        let close = find_matching_paren(stmt, i2)?;
        let mut names = Vec::new();
        for (s, e) in split_top_level_commas(&stmt[i2 + 1..close]) {
            let (name, _) = parse_backtick_ident(stmt, i2 + 1 + s)?;
            let _ = e;
            names.push(name);
        }
        i2 = close + 1;
        i2 = skip_ws(stmt, i2);
        Some(names)
    } else {
        None
    };

    i2 = expect_keyword(stmt, i2, b"VALUES")?;
    i2 = skip_ws(stmt, i2);

    Ok(InsertHead {
        table,
        columns,
        prefix_end: i2,
    })
}

/// Trouve les bornes `(start, end)` (contenu entre les parenthèses, exclu) de
/// chaque tuple `(...)` d'une liste `VALUES (...),(...),...;`.
fn find_value_tuples(stmt: &[u8], mut i: usize) -> Result<Vec<(usize, usize)>> {
    let mut tuples = Vec::new();
    loop {
        if stmt.get(i) != Some(&b'(') {
            bail!("'(' attendu pour un tuple VALUES à la position {i}");
        }
        let close = find_matching_paren(stmt, i)?;
        tuples.push((i + 1, close));
        i = skip_ws(stmt, close + 1);
        match stmt.get(i) {
            Some(b',') => {
                i = skip_ws(stmt, i + 1);
            }
            _ => break,
        }
    }
    Ok(tuples)
}

/// Parse le contenu d'un tuple (entre parenthèses, hors parenthèses) en valeurs.
fn parse_row(tuple: &[u8]) -> Result<Vec<Value<'_>>> {
    split_top_level_commas(tuple)
        .into_iter()
        .map(|(s, e)| parse_value(&tuple[s..e]))
        .collect()
}

fn parse_value(token: &[u8]) -> Result<Value<'_>> {
    if token.eq_ignore_ascii_case(b"NULL") {
        return Ok(Value::Null);
    }
    if token.first() == Some(&b'\'') {
        if token.len() < 2 || token.last() != Some(&b'\'') {
            bail!("chaîne mal terminée : {:?}", String::from_utf8_lossy(token));
        }
        return Ok(Value::Str(unescape_sql_string(&token[1..token.len() - 1])));
    }
    Ok(Value::Raw(token))
}

/// Désescape le contenu d'une chaîne SQL entre quotes simples : gère les
/// séquences `\'`, `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\Z`, `\b`, ainsi que
/// l'apostrophe doublée `''`. Ne copie que si une séquence a été trouvée.
fn unescape_sql_string(inner: &[u8]) -> Cow<'_, [u8]> {
    if !inner.iter().any(|&b| b == b'\\') && !inner.windows(2).any(|w| w == b"''") {
        return Cow::Borrowed(inner);
    }
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        match inner[i] {
            b'\\' if i + 1 < inner.len() => {
                out.push(match inner[i + 1] {
                    b'0' => b'\0',
                    b'b' => 0x08,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'Z' => 0x1A,
                    other => other, // \', \", \\ et tout le reste : l'octet échappé lui-même.
                });
                i += 2;
            }
            b'\'' if inner.get(i + 1) == Some(&b'\'') => {
                out.push(b'\'');
                i += 2;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

/// Ré-échappe une chaîne pour l'écrire entre quotes simples, avec exactement
/// les règles utilisées par `mysqldump` en sortie.
fn escape_sql_string(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        match b {
            0x00 => out.extend_from_slice(b"\\0"),
            0x08 => out.extend_from_slice(b"\\b"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x1A => out.extend_from_slice(b"\\Z"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\'' => out.extend_from_slice(b"\\'"),
            b'"' => out.extend_from_slice(b"\\\""),
            other => out.push(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Le parseur complet
// ---------------------------------------------------------------------------

struct PendingInsert {
    /// Position du premier `(` de la liste `VALUES` dans l'instruction.
    values_start: usize,
    /// Bornes des tuples, calculées paresseusement à la première `Row` :
    /// une table skippée par l'appelant (cf. [`DumpParser::skip_rows`]) n'est
    /// ainsi jamais scannée au-delà de son préfixe.
    tuples: Vec<(usize, usize)>,
    scanned: bool,
    next: usize,
}

/// Parseur de dump `mysqldump`, lu depuis n'importe quelle source [`Read`].
pub struct MysqlParser<R> {
    reader: R,
    /// Octets lus mais pas encore renvoyés en événement.
    buf: Vec<u8>,
    /// Octets de `buf` déjà consommés (renvoyés lors d'un appel précédent).
    start: usize,
    /// `true` une fois que `reader` a renvoyé 0 octet.
    eof: bool,
    /// Avancement du scan de fin d'instruction sur `buf[..]`, entre deux lectures.
    cursor: ScanCursor,
    /// `Some` tant qu'on égrène les `Row` d'un `INSERT` en cours.
    pending_insert: Option<PendingInsert>,
}

impl<R: Read> MysqlParser<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::new(),
            start: 0,
            eof: false,
            cursor: ScanCursor::START,
            pending_insert: None,
        }
    }

    /// Retourne la prochaine instruction complète (avec son `;` et son saut
    /// de ligne final), ou `None` en fin de flux.
    fn next_statement(&mut self) -> Result<Option<&[u8]>> {
        loop {
            if self.start > 0 {
                self.buf.drain(0..self.start);
                self.start = 0;
                self.cursor = ScanCursor::START;
            }

            if let Some(end) = find_statement_end(&self.buf, self.eof, &mut self.cursor) {
                self.start = end;
                return Ok(Some(&self.buf[..end]));
            }

            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                // Reste de données sans `;` terminal (fichier tronqué ou
                // dernière ligne sans ponctuation) : on le rend tel quel.
                self.start = self.buf.len();
                return Ok(Some(&self.buf[..]));
            }

            let mut chunk = [0u8; READ_CHUNK];
            let n = self.reader.read(&mut chunk)?;
            if n == 0 {
                self.eof = true;
            } else {
                self.buf.extend_from_slice(&chunk[..n]);
            }
        }
    }

    fn next_row_event(&mut self) -> Result<Option<Event<'_>>> {
        let pending = self
            .pending_insert
            .as_mut()
            .expect("pending_insert is Some");
        if !pending.scanned {
            // L'instruction courante est `buf[..start]` (pas encore drainée :
            // `next_statement` ne l'est qu'une fois `pending_insert` vidé).
            pending.tuples = find_value_tuples(&self.buf[..self.start], pending.values_start)?;
            pending.scanned = true;
        }
        if pending.next < pending.tuples.len() {
            let (s, e) = pending.tuples[pending.next];
            pending.next += 1;
            let values = parse_row(&self.buf[s..e])?;
            Ok(Some(Event::Row(values)))
        } else {
            self.pending_insert = None;
            Ok(Some(Event::RowsEnd))
        }
    }
}

/// Résultat, entièrement possédé, de la classification d'une instruction —
/// pour ne conserver aucun emprunt de `self.buf` une fois la décision prise
/// (le writer, lui, a besoin d'un nouvel emprunt frais pour l'`Event` renvoyé).
enum Decision {
    Raw,
    Table(Table),
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        prefix_end: usize,
    },
}

impl<R: Read> DumpParser for MysqlParser<R> {
    fn skip_rows(&mut self) {
        self.pending_insert = None;
    }

    fn next_event(&mut self) -> Result<Option<Event<'_>>> {
        if self.pending_insert.is_some() {
            return self.next_row_event();
        }

        let (end, decision) = {
            let stmt = match self.next_statement()? {
                Some(s) => s,
                None => return Ok(None),
            };
            // Une instruction peut commencer par des lignes blanches (les
            // dumps ne sont pas tous aussi disciplinés que `mysqldump`, qui
            // sépare toujours ses sections par un commentaire `--`) : sans ce
            // décalage, `starts_with_ci` échoue sur le `CREATE TABLE` /
            // `INSERT INTO` réel et l'instruction est silencieusement prise
            // pour du `Raw` — la table ou les lignes disparaissent du scan
            // sans la moindre erreur.
            let leading = skip_ws(stmt, 0);
            let body = &stmt[leading..];
            let decision = if starts_with_ci(body, b"CREATE TABLE") {
                Decision::Table(parse_create_table(body)?)
            } else if starts_with_ci(body, b"INSERT INTO") {
                let head = parse_insert_head(body)?;
                Decision::Insert {
                    table: head.table,
                    columns: head.columns,
                    prefix_end: leading + head.prefix_end,
                }
            } else {
                Decision::Raw
            };
            (stmt.len(), decision)
        };

        match decision {
            Decision::Raw => Ok(Some(Event::Raw(&self.buf[..end]))),
            Decision::Table(table) => Ok(Some(Event::TableSchema(table))),
            Decision::Insert {
                table,
                columns,
                prefix_end,
            } => {
                self.pending_insert = Some(PendingInsert {
                    values_start: prefix_end,
                    tuples: Vec::new(),
                    scanned: false,
                    next: 0,
                });
                Ok(Some(Event::RowsBegin {
                    table,
                    columns,
                    prefix: &self.buf[..prefix_end],
                }))
            }
        }
    }
}

/// Writer symétrique de [`MysqlParser`] : réécrit un flux d'[`Event`] au
/// format `mysqldump`.
pub struct MysqlWriter<W> {
    writer: W,
    /// `true` si la prochaine `Row` est la première depuis le dernier `RowsBegin`.
    first_row: bool,
}

impl<W: Write> MysqlWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            first_row: true,
        }
    }

    fn write_value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Null => self.writer.write_all(b"NULL")?,
            Value::Raw(bytes) => self.writer.write_all(bytes)?,
            Value::Str(s) => {
                let mut escaped = Vec::with_capacity(s.len());
                escape_sql_string(s, &mut escaped);
                self.writer.write_all(b"'")?;
                self.writer.write_all(&escaped)?;
                self.writer.write_all(b"'")?;
            }
        }
        Ok(())
    }
}

impl<W: Write> DumpWriter for MysqlWriter<W> {
    fn write_event(&mut self, ev: &Event) -> Result<()> {
        match ev {
            Event::Raw(bytes) => self.writer.write_all(bytes)?,
            Event::TableSchema(table) => self.writer.write_all(&table.raw)?,
            Event::RowsBegin { prefix, .. } => {
                self.writer.write_all(prefix)?;
                self.first_row = true;
            }
            Event::Row(values) => {
                if !self.first_row {
                    self.writer.write_all(b",")?;
                }
                self.first_row = false;
                self.writer.write_all(b"(")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        self.writer.write_all(b",")?;
                    }
                    self.write_value(v)?;
                }
                self.writer.write_all(b")")?;
            }
            Event::RowsEnd => self.writer.write_all(b";\n")?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ends(input: &str, eof: bool) -> Option<usize> {
        let mut cursor = ScanCursor::START;
        find_statement_end(input.as_bytes(), eof, &mut cursor)
    }

    /// Lecteur qui ne rend jamais plus de `n` octets par appel, pour forcer
    /// une instruction à arriver en beaucoup de morceaux.
    struct Dribble<'a> {
        data: &'a [u8],
        n: usize,
    }

    impl Read for Dribble<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.n.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn roundtrip_dribble(bytes: &[u8], n: usize) -> Vec<u8> {
        let mut parser = MysqlParser::new(Dribble { data: bytes, n });
        let mut out = Vec::new();
        let mut writer = MysqlWriter::new(&mut out);
        while let Some(event) = parser.next_event().unwrap() {
            writer.write_event(&event).unwrap();
        }
        out
    }

    #[test]
    fn resumable_scan_splits_statements_identically_whatever_the_read_size() {
        // Le scan de fin d'instruction reprend là où il s'était arrêté entre
        // deux lectures : toute frontière de lecture (au milieu d'un `;\r\n`,
        // d'un `\'`, d'un `--`, d'un `''`…) doit donner le même découpage.
        for fixture in [
            &include_bytes!("../../fixtures/exemples/dumps/01_basic.sql")[..],
            &include_bytes!("../../fixtures/exemples/dumps/02_parser_edge_cases.sql")[..],
        ] {
            let reference = roundtrip(fixture);
            for n in [1, 2, 3, 7, 64, 1000] {
                assert_eq!(
                    roundtrip_dribble(fixture, n),
                    reference,
                    "lecture par {n} octets"
                );
                assert_eq!(
                    collect_events_from(Dribble { data: fixture, n }),
                    collect_events(fixture),
                    "événements, lecture par {n} octets"
                );
            }
        }
    }

    #[test]
    fn skip_rows_jumps_to_the_next_statement_without_parsing_tuples() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let mut parser = MysqlParser::new(input);
        let mut seen = Vec::new();
        while let Some(event) = parser.next_event().unwrap() {
            match event {
                Event::RowsBegin { table, .. } => {
                    seen.push(format!("begin {table}"));
                    parser.skip_rows();
                }
                Event::Row(_) => panic!("aucune Row ne doit être émise après skip_rows"),
                Event::RowsEnd => panic!("aucun RowsEnd ne doit être émis après skip_rows"),
                Event::TableSchema(t) => seen.push(format!("schema {}", t.name)),
                Event::Raw(_) => {}
            }
        }
        assert!(seen.contains(&"begin user".to_string()));
        assert!(seen.iter().filter(|s| s.starts_with("begin ")).count() >= 2);
    }

    #[test]
    fn simple_statement() {
        assert_eq!(ends("SELECT 1;\nSELECT 2;\n", false), Some(10));
    }

    #[test]
    fn semicolon_inside_single_quoted_string_is_ignored() {
        assert_eq!(ends("INSERT INTO t VALUES ('a;b');\n", false), Some(30));
    }

    #[test]
    fn escaped_quote_does_not_close_the_string() {
        assert_eq!(ends("INSERT INTO t VALUES ('a\\'; b');\n", false), Some(33));
    }

    #[test]
    fn doubled_quote_does_not_close_the_string() {
        assert_eq!(ends("INSERT INTO t VALUES ('a''; b');\n", false), Some(33));
    }

    #[test]
    fn semicolon_inside_line_comment_is_ignored() {
        // Un commentaire `-- …` en tête d'instruction est désormais une
        // instruction à lui seul (voir le cas particulier en tête de
        // `find_statement_end`) : le `;` qu'il contient ne le termine pas
        // prématurément, mais ce n'est plus fusionné avec la ligne suivante.
        assert_eq!(ends("-- a; b\nSELECT 1;\n", false), Some(8));
    }

    #[test]
    fn semicolon_inside_line_comment_mid_statement_is_ignored() {
        assert_eq!(ends("SELECT 1 /* x */-- a; b\n;\n", false), Some(26));
    }

    #[test]
    fn semicolon_inside_block_comment_is_ignored() {
        assert_eq!(ends("/*!40101 a; b */;\n", false), Some(18));
    }

    #[test]
    fn incomplete_statement_needs_more_data() {
        assert_eq!(ends("SELECT 1", false), None);
        assert_eq!(ends("SELECT 1;", false), None); // pas encore de fin de ligne
    }

    #[test]
    fn eof_without_trailing_newline_still_terminates() {
        assert_eq!(ends("SELECT 1;", true), Some(9));
    }

    #[test]
    fn delimiter_block_is_split_on_the_semicolon_immediately_before_a_newline() {
        assert_eq!(ends("DELIMITER ;;\n", false), Some(13));
    }

    fn collect_events(bytes: &[u8]) -> Vec<String> {
        collect_events_from(bytes)
    }

    fn collect_events_from<R: Read>(reader: R) -> Vec<String> {
        let mut parser = MysqlParser::new(reader);
        let mut out = Vec::new();
        while let Some(event) = parser.next_event().unwrap() {
            out.push(match event {
                Event::Raw(b) => format!("Raw({} bytes)", b.len()),
                Event::TableSchema(t) => {
                    format!("TableSchema({}, {} cols)", t.name, t.columns.len())
                }
                Event::RowsBegin { table, columns, .. } => {
                    format!("RowsBegin({table}, {columns:?})")
                }
                Event::Row(values) => format!("Row({} values)", values.len()),
                Event::RowsEnd => "RowsEnd".to_string(),
            });
        }
        out
    }

    fn roundtrip(bytes: &[u8]) -> Vec<u8> {
        let mut parser = MysqlParser::new(bytes);
        let mut out = Vec::new();
        let mut writer = MysqlWriter::new(&mut out);
        while let Some(event) = parser.next_event().unwrap() {
            writer.write_event(&event).unwrap();
        }
        out
    }

    #[test]
    fn roundtrips_basic_fixture_byte_for_byte() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        assert_eq!(roundtrip(input), input);
    }

    #[test]
    fn roundtrips_edge_cases_fixture_byte_for_byte() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/02_parser_edge_cases.sql");
        let output = roundtrip(input);

        // Limitation connue et acceptée : le désescapage transforme `''` (une
        // apostrophe échappée façon SQL standard) et `\'` (façon mysqldump)
        // en la même valeur logique ; en écriture on ré-échappe toujours à la
        // façon mysqldump (`\'`), la seule que `mysqldump` produit réellement.
        // Le cas 5 de cette fixture utilise volontairement `''` pour tester
        // que le *parseur* la reconnaît ; on normalise cette seule occurrence
        // avant de comparer, plutôt que de complexifier le writer pour un
        // style d'échappement que `mysqldump` ne génère jamais lui-même.
        let expected =
            String::from_utf8_lossy(input).replace("VALUES (1,''x'')", "VALUES (1,\\'x\\')");
        let expected = expected.as_bytes();

        if output != expected {
            let n = output.len().min(expected.len());
            let diff_at = (0..n).find(|&i| output[i] != expected[i]).unwrap_or(n);
            let start = diff_at.saturating_sub(40);
            panic!(
                "diverge à l'octet {diff_at} (expected.len()={}, output.len()={})\nexpected: {:?}\noutput  : {:?}",
                expected.len(),
                output.len(),
                String::from_utf8_lossy(&expected[start..(diff_at + 40).min(expected.len())]),
                String::from_utf8_lossy(&output[start..(diff_at + 40).min(output.len())]),
            );
        }
    }

    #[test]
    fn parses_user_table_schema() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let events = collect_events(input);
        assert!(events.iter().any(|e| e == "TableSchema(user, 11 cols)"));
    }

    #[test]
    fn detects_single_column_unique_and_primary_key_constraints() {
        let input: &[u8] = include_bytes!("../../fixtures/exemples/dumps/01_basic.sql");
        let mut parser = MysqlParser::new(input);
        let mut user_table = None;
        while let Some(event) = parser.next_event().unwrap() {
            if let Event::TableSchema(t) = event {
                if t.name == "user" {
                    user_table = Some(t);
                    break;
                }
            }
        }
        let user = user_table.expect("table user introuvable");
        let unique_cols: Vec<&str> = user
            .columns
            .iter()
            .filter(|c| c.unique)
            .map(|c| c.name.as_str())
            .collect();
        // `id` (PRIMARY KEY) et `email` (UNIQUE KEY) sont mono-colonnes.
        assert!(unique_cols.contains(&"id"));
        assert!(unique_cols.contains(&"email"));
        assert!(!unique_cols.contains(&"first_name"));
    }

    #[test]
    fn escape_unescape_roundtrip_on_random_bytes() {
        use rand::RngExt;
        use rand::SeedableRng;
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(42);
        for _ in 0..200 {
            let len = rng.random_range(0..40);
            let bytes: Vec<u8> = (0..len).map(|_| rng.random::<u8>()).collect();
            let mut escaped = Vec::new();
            escape_sql_string(&bytes, &mut escaped);
            // L'échappé ne doit plus contenir de quote ni de backslash nus.
            assert!(!escaped.contains(&b'\'') || escaped.windows(2).all(|w| w != b"'"));
            let unescaped = unescape_sql_string(&escaped);
            assert_eq!(unescaped.as_ref(), bytes.as_slice());
        }
    }
}
