use std::borrow::Cow;

use anyhow::Result;

pub mod mysql;

/// A column as declared in a `CREATE TABLE`.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub sql_type: SqlType,
    pub nullable: bool,
    pub max_len: Option<u32>,
    pub generated: bool,
    /// `true` if this column alone is covered by a `PRIMARY KEY` or `UNIQUE KEY`
    /// constraint. Composite keys don't count: none of their columns is unique
    /// on its own.
    pub unique: bool,
}

/// A table as declared in a `CREATE TABLE`.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// Raw `FOREIGN KEY` definitions, kept for v0.2 (no cost here).
    pub foreign_keys: Vec<String>,
    /// The full `CREATE TABLE` statement as read from the dump: it is always
    /// rewritten verbatim, never regenerated.
    pub raw: Vec<u8>,
}

/// SQL type of a column, as extracted from its `CREATE TABLE` declaration.
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

/// A raw value read from a `VALUES` tuple.
#[derive(Debug, Clone)]
pub enum Value<'a> {
    /// The SQL `NULL` literal.
    Null,

    /// Anything that is not an SQL string (numbers, `0x…`, `_binary '…'`,
    /// `b'…'`). Copied verbatim, never transformed.
    Raw(&'a [u8]),

    /// Unescaped content of a `'…'` string. The serializer re-escapes it.
    Str(Cow<'a, [u8]>),
}

/// An event produced by a [`DumpParser`] and consumed by a [`DumpWriter`].
pub enum Event<'a> {
    /// Anything that is neither a `CREATE TABLE` nor an `INSERT INTO`, rewritten verbatim.
    Raw(&'a [u8]),
    /// A `CREATE TABLE`; the parser also keeps it as `Raw` to rewrite it verbatim.
    TableSchema(Table),
    /// Start of an `INSERT INTO` block.
    RowsBegin {
        table: String,
        /// `Some` if the `INSERT` has an explicit column list (generated columns).
        columns: Option<Vec<String>>,
        /// Raw `INSERT INTO ... VALUES ` text (up to, but excluding, the first `(`),
        /// kept to rewrite the prefix verbatim.
        prefix: &'a [u8],
    },
    /// A value tuple, ordered as `RowsBegin.columns` if present, otherwise as the table.
    Row(Vec<Value<'a>>),
    /// End of the `INSERT INTO` block opened by `RowsBegin`.
    RowsEnd,
}

pub trait DumpParser {
    /// Reads and returns the next event, or `None` at end of stream.
    fn next_event(&mut self) -> Result<Option<Event<'_>>>;

    /// Discards the `Row`s of the `INSERT` block opened by the last `RowsBegin`
    /// (no `Row` or `RowsEnd` is emitted for it): the next event is the
    /// following statement. Avoids parsing tuples of a table the caller won't
    /// write.
    fn skip_rows(&mut self);
}

pub trait DumpWriter {
    /// Writes an event to the output stream.
    fn write_event(&mut self, ev: &Event) -> Result<()>;
}
