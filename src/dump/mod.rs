use std::borrow::Cow;

use anyhow::Result;

pub mod mysql;

/// Une colonne telle que déclarée dans un `CREATE TABLE`.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub sql_type: SqlType,
    pub nullable: bool,
    pub max_len: Option<u32>,
    pub generated: bool,
}

/// Une table telle que déclarée dans un `CREATE TABLE`.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// Définitions `FOREIGN KEY` brutes, conservées pour la v0.2 (coût nul ici).
    pub foreign_keys: Vec<String>,
    /// L'instruction `CREATE TABLE` complète, telle que lue dans le dump :
    /// on ne régénère jamais un `CREATE TABLE`, on le réécrit à l'identique.
    pub raw: Vec<u8>,
}

/// Type SQL d'une colonne, tel qu'extrait de sa déclaration `CREATE TABLE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlType {
    Int,
    Decimal,
    Float,
    Char,
    Text,
    Blob,
    Date,
    DateTime,
    Json,
    Enum,
    Set,
    Bit,
    Other(String),
}

/// Une valeur brute lue depuis un tuple de `VALUES`.
#[derive(Debug, Clone)]
pub enum Value<'a> {
    /// Le littéral SQL `NULL`.
    Null,

    /// Tout ce qui n'est pas une chaîne SQL (nombres, `0x…`, `_binary '…'`,
    /// `b'…'`). Copié tel quel, jamais transformé.
    Raw(&'a [u8]),

    /// Contenu désescapé d'une chaîne `'…'`. Le sérialiseur ré-échappe.
    Str(Cow<'a, [u8]>),
}

/// Un événement produit par un [`DumpParser`] et consommé par un [`DumpWriter`].
pub enum Event<'a> {
    /// Tout ce qui n'est ni un `CREATE TABLE` ni un `INSERT INTO`, réécrit tel quel.
    Raw(&'a [u8]),
    /// Un `CREATE TABLE`, conservé aussi en `Raw` par le parseur pour être réécrit à l'identique.
    TableSchema(Table),
    /// Début d'un bloc `INSERT INTO`.
    RowsBegin {
        table: String,
        /// `Some` si l'`INSERT` a une liste de colonnes explicite (cas des colonnes générées).
        columns: Option<Vec<String>>,
        /// Texte brut de `INSERT INTO ... VALUES ` (jusqu'au premier `(` inclus
        /// exclu), conservé pour réécrire le préfixe à l'identique.
        prefix: &'a [u8],
    },
    /// Un tuple de valeurs, dans l'ordre de `RowsBegin.columns` si présent, sinon celui de la table.
    Row(Vec<Value<'a>>),
    /// Fin du bloc `INSERT INTO` ouvert par `RowsBegin`.
    RowsEnd,
}

pub trait DumpParser {
    /// Lit et retourne le prochain événement, ou `None` en fin de flux.
    fn next_event(&mut self) -> Result<Option<Event<'_>>>;
}

pub trait DumpWriter {
    /// Écrit un événement dans le flux de sortie.
    fn write_event(&mut self, ev: &Event) -> Result<()>;
}
