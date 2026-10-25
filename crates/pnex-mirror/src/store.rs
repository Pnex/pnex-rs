//! The SQLite copy: objects, links and media as PneX sends them, the
//! properties and attributes as JSON, no business projection. The cursor
//! of the feed is written in the same transaction as the rows it covers.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Deserialize;
use serde_json::{Map, Value};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS objects (
    id TEXT PRIMARY KEY,
    type_key TEXT NOT NULL,
    title TEXT NOT NULL,
    native_id TEXT,
    version INTEGER NOT NULL,
    properties TEXT NOT NULL,
    source_ref TEXT,
    valid_from TEXT,
    valid_to TEXT,
    updated_at TEXT
);
CREATE INDEX IF NOT EXISTS objects_type ON objects (type_key);
CREATE TABLE IF NOT EXISTS links (
    id INTEGER PRIMARY KEY,
    link_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    attributes TEXT NOT NULL,
    source_ref TEXT,
    valid_from TEXT,
    valid_to TEXT
);
CREATE INDEX IF NOT EXISTS links_source ON links (source_id, link_type);
CREATE INDEX IF NOT EXISTS links_target ON links (target_id, link_type);
CREATE TABLE IF NOT EXISTS media (
    asset_id TEXT PRIMARY KEY,
    object_id TEXT NOT NULL,
    kind TEXT,
    size_bytes INTEGER,
    version INTEGER,
    sha256 TEXT,
    content_type TEXT,
    etag TEXT,
    wanted INTEGER,
    fetched_at TEXT
);
CREATE TABLE IF NOT EXISTS mirror_state (k TEXT PRIMARY KEY, v TEXT NOT NULL);
";

/// What happened to a row, as the feed says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Create,
    Update,
    Close,
    Delete,
}

/// An object as PneX sends it (`ObjectView`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ObjectRow {
    pub id: String,
    pub type_key: String,
    pub title: String,
    #[serde(default)]
    pub native_id: Option<String>,
    pub version: i64,
    #[serde(default)]
    pub properties: Map<String, Value>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct End {
    pub id: String,
}

/// A link as PneX sends it (`LinkView`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LinkRow {
    pub id: i64,
    pub link_type: String,
    pub source: End,
    pub target: End,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
}

/// What an application projects into its own tables, in the transaction
/// that applies a page of the feed (an error rolls the page back).
pub trait Projection {
    /// An object written or removed (`Delete`: `o` is the last copy).
    fn object(
        &mut self,
        _tx: &Transaction,
        _o: &ObjectRow,
        _action: Action,
    ) -> rusqlite::Result<()> {
        Ok(())
    }
    /// A link written or removed.
    fn link(&mut self, _tx: &Transaction, _l: &LinkRow, _action: Action) -> rusqlite::Result<()> {
        Ok(())
    }
    /// The copy restarts from nothing (cursor expired): drop what you built.
    fn reset(&mut self, _tx: &Transaction) -> rusqlite::Result<()> {
        Ok(())
    }
}

/// No projection: the mirror tables only.
pub struct NoProjection;
impl Projection for NoProjection {}

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

pub fn state(c: &Connection, k: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT v FROM mirror_state WHERE k = ?1", [k], |r| r.get(0))
        .optional()
}

pub fn set_state(tx: &Transaction, k: &str, v: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO mirror_state (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
        [k, v],
    )?;
    Ok(())
}

pub const MEDIA_TYPE: &str = "media_asset";

pub fn put_object(tx: &Transaction, o: &ObjectRow) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO objects (id, type_key, title, native_id, version, properties, source_ref, valid_from, valid_to, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT (id) DO UPDATE SET type_key = excluded.type_key, title = excluded.title, \
           native_id = excluded.native_id, version = excluded.version, properties = excluded.properties, \
           source_ref = excluded.source_ref, valid_from = excluded.valid_from, valid_to = excluded.valid_to, \
           updated_at = excluded.updated_at",
        params![
            o.id,
            o.type_key,
            o.title,
            o.native_id,
            o.version,
            Value::Object(o.properties.clone()).to_string(),
            o.source_ref,
            o.valid_from,
            o.valid_to,
            o.updated_at,
        ],
    )?;
    if o.type_key == MEDIA_TYPE {
        if let Some(asset) = o.native_id.as_deref() {
            tx.execute(
                "INSERT INTO media (asset_id, object_id) VALUES (?1, ?2) ON CONFLICT (asset_id) DO NOTHING",
                [asset, o.id.as_str()],
            )?;
        }
    }
    Ok(())
}

pub fn remove_object(tx: &Transaction, id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM media WHERE object_id = ?1", [id])?;
    tx.execute("DELETE FROM objects WHERE id = ?1", [id])?;
    Ok(())
}

pub fn put_link(tx: &Transaction, l: &LinkRow) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO links (id, link_type, source_id, target_id, attributes, source_ref, valid_from, valid_to) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (id) DO UPDATE SET link_type = excluded.link_type, source_id = excluded.source_id, \
           target_id = excluded.target_id, attributes = excluded.attributes, source_ref = excluded.source_ref, \
           valid_from = excluded.valid_from, valid_to = excluded.valid_to",
        params![
            l.id,
            l.link_type,
            l.source.id,
            l.target.id,
            Value::Object(l.attributes.clone()).to_string(),
            l.source_ref,
            l.valid_from,
            l.valid_to,
        ],
    )?;
    Ok(())
}

pub fn remove_link(tx: &Transaction, id: i64) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM links WHERE id = ?1", [id])?;
    Ok(())
}

/// Everything copied goes (the files by hash stay, reused when they match).
pub fn wipe(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute_batch("DELETE FROM links; DELETE FROM media; DELETE FROM objects;")
}

fn object_of(r: &rusqlite::Row) -> rusqlite::Result<ObjectRow> {
    let props: String = r.get(5)?;
    Ok(ObjectRow {
        id: r.get(0)?,
        type_key: r.get(1)?,
        title: r.get(2)?,
        native_id: r.get(3)?,
        version: r.get(4)?,
        properties: serde_json::from_str(&props).unwrap_or_default(),
        source_ref: r.get(6)?,
        valid_from: r.get(7)?,
        valid_to: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

const OBJECT_COLUMNS: &str =
    "id, type_key, title, native_id, version, properties, source_ref, valid_from, valid_to, updated_at";

pub fn object(c: &Connection, id: &str) -> rusqlite::Result<Option<ObjectRow>> {
    c.query_row(
        &format!("SELECT {OBJECT_COLUMNS} FROM objects WHERE id = ?1"),
        [id],
        object_of,
    )
    .optional()
}

/// Live objects of a type, by title.
pub fn objects(c: &Connection, type_key: &str) -> rusqlite::Result<Vec<ObjectRow>> {
    let mut st = c.prepare(&format!(
        "SELECT {OBJECT_COLUMNS} FROM objects WHERE type_key = ?1 AND valid_to IS NULL ORDER BY title, id"
    ))?;
    let rows = st.query_map([type_key], object_of)?;
    rows.collect()
}

/// Open links from an object, of one type or all.
pub fn links_from(
    c: &Connection,
    id: &str,
    link_type: Option<&str>,
) -> rusqlite::Result<Vec<LinkRow>> {
    let mut st = c.prepare(
        "SELECT id, link_type, source_id, target_id, attributes, source_ref, valid_from, valid_to FROM links \
         WHERE source_id = ?1 AND (?2 IS NULL OR link_type = ?2) AND valid_to IS NULL ORDER BY id",
    )?;
    let rows = st.query_map(params![id, link_type], |r| {
        let attrs: String = r.get(4)?;
        Ok(LinkRow {
            id: r.get(0)?,
            link_type: r.get(1)?,
            source: End { id: r.get(2)? },
            target: End { id: r.get(3)? },
            attributes: serde_json::from_str(&attrs).unwrap_or_default(),
            source_ref: r.get(5)?,
            valid_from: r.get(6)?,
            valid_to: r.get(7)?,
        })
    })?;
    rows.collect()
}
