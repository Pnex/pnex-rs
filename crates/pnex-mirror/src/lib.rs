//! # pnex-mirror
//!
//! The offline copy of a PneX organization for applications built on its
//! ontology (external-apps.md D439). It knows no type, link or pack: the
//! application says what to copy, or takes the scope PneX declared for it.
//!
//! - **Feed**: follows `GET /api/v1/ontology/changes` from its cursor; the
//!   cursor and the rows of a page are written in one SQLite transaction,
//!   so a copy cut at any moment resumes where it stopped. An expired
//!   cursor (410) empties the copy and starts again from 0.
//! - **Mirror**: objects and links as PneX sends them, their properties
//!   and attributes as JSON (`objects`, `links` tables).
//! - **Media**: the `media_asset` objects copied, downloaded in the
//!   background by kind and size, checked against their SHA-256 and stored
//!   by hash (`<dir>/ab/cd/<sha256>`, no duplicate). Every pass revalidates
//!   with `If-None-Match` (a new version arrives, a 304 costs nothing); the
//!   last valid file stays when PneX is down.
//! - **Projection**: [`Projection`] callbacks run in the transaction of
//!   each page, for the application's own views; or plain SQL on
//!   [`Mirror::connection`].
//!
//! ```no_run
//! # async fn run() -> pnex_mirror::Result<()> {
//! let config = pnex_mirror::Config::new("https://pnex.example", "pnex_sk_…", "./copy")
//!     .from_app()
//!     .await?;
//! let mirror = pnex_mirror::Mirror::open(config)?;
//! mirror.sync(&mut pnex_mirror::NoProjection).await?;
//! mirror.fetch_media().await?;
//! for article in mirror.objects("article")? {
//!     println!("{} {}", article.title, article.properties["lang"]);
//! }
//! # Ok(()) }
//! ```

mod client;
mod store;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use client::{Client, Error, Fetched, Result};
pub use store::{Action, End, LinkRow, NoProjection, ObjectRow, Projection};

/// What to copy and where.
#[derive(Debug, Clone)]
pub struct Config {
    /// Server root, `https://pnex.example`.
    pub base_url: String,
    /// The service account's access key (`pnex_sk_…`).
    pub api_key: String,
    /// The org, when the key's own org is not meant (rarely needed).
    pub org_id: Option<i64>,
    pub db_path: PathBuf,
    pub media_dir: PathBuf,
    /// Object types to copy; `None` = all the feed gives.
    pub types: Option<Vec<String>>,
    /// Link types to copy; `None` = all the feed gives.
    pub link_types: Option<Vec<String>>,
    /// Media kinds to download (`photo`, `document`…); empty = none.
    pub media_kinds: Vec<String>,
    /// Largest media to download, in bytes.
    pub media_max_bytes: Option<u64>,
    /// Feed entries per request (1 to 1000).
    pub page_size: u32,
}

impl Config {
    /// A copy under `dir` (`mirror.sqlite`, `media/`), everything the key
    /// may read, no media.
    pub fn new(base_url: &str, api_key: &str, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Self {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            org_id: None,
            db_path: dir.join("mirror.sqlite"),
            media_dir: dir.join("media"),
            types: None,
            link_types: None,
            media_kinds: Vec::new(),
            media_max_bytes: None,
            page_size: 500,
        }
    }

    /// Takes what to copy from the application's declaration in PneX
    /// (`GET /api/v1/apps/me`): its types, link types and media.
    pub async fn from_app(mut self) -> Result<Self> {
        #[derive(Deserialize)]
        struct Scope {
            #[serde(default)]
            types: Vec<String>,
            #[serde(default)]
            link_types: Vec<String>,
            #[serde(default)]
            media_kinds: Vec<String>,
            #[serde(default)]
            media_max_bytes: Option<u64>,
        }
        #[derive(Deserialize)]
        struct App {
            scope: Scope,
        }
        let app: App = self.client()?.get_json("/api/v1/apps/me").await?;
        self.types = Some(app.scope.types);
        self.link_types = Some(app.scope.link_types);
        self.media_kinds = app.scope.media_kinds;
        self.media_max_bytes = app.scope.media_max_bytes;
        Ok(self)
    }

    pub fn client(&self) -> Result<Client> {
        Client::new(&self.base_url, &self.api_key, self.org_id)
    }
}

#[derive(Deserialize)]
struct Feed {
    changes: Vec<Entry>,
    next: String,
    more: bool,
}

#[derive(Deserialize)]
struct Entry {
    kind: String,
    action: Action,
    id: String,
    #[serde(default)]
    object: Option<ObjectRow>,
    #[serde(default)]
    link: Option<LinkRow>,
}

/// What a sync applied.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SyncReport {
    pub objects: usize,
    pub links: usize,
    /// The copy restarted from 0 (cursor expired).
    pub reset: bool,
    pub cursor: String,
}

/// What a media pass did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MediaReport {
    pub downloaded: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub failed: usize,
}

const CURSOR: &str = "cursor";

pub struct Mirror {
    client: Client,
    config: Config,
    db: Mutex<rusqlite::Connection>,
}

fn list(l: &Option<Vec<String>>) -> String {
    l.as_ref().map(|v| v.join(",")).unwrap_or_default()
}

impl Mirror {
    pub fn open(config: Config) -> Result<Self> {
        if let Some(dir) = config.db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(&config.media_dir)?;
        Ok(Self {
            client: config.client()?,
            db: Mutex::new(store::open(&config.db_path)?),
            config,
        })
    }

    /// The SQLite copy, for the application's own queries. Keep the guard
    /// short: a sync waits for it.
    pub fn connection(&self) -> MutexGuard<'_, rusqlite::Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cursor of the last page applied (`0` before the first).
    pub fn cursor(&self) -> Result<String> {
        Ok(store::state(&self.connection(), CURSOR)?.unwrap_or_else(|| "0".into()))
    }

    /// Applies the feed until nothing is left; `projection` sees each row.
    pub async fn sync(&self, projection: &mut dyn Projection) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        loop {
            let since = self.cursor()?;
            let mut path = format!(
                "/api/v1/ontology/changes?since={since}&limit={}",
                self.config.page_size.clamp(1, 1000)
            );
            // An empty list means none of that kind, which the server reads
            // as "no filter": send a key no type has.
            for (param, l) in [
                ("types", &self.config.types),
                ("link_types", &self.config.link_types),
            ] {
                if l.is_some() {
                    let v = list(l);
                    path.push_str(&format!("&{param}={}", if v.is_empty() { "-" } else { &v }));
                }
            }
            let feed: Feed = match self.client.get_json(&path).await {
                Err(e) if e.status() == Some(410) => {
                    let mut c = self.connection();
                    let tx = c.transaction()?;
                    store::wipe(&tx)?;
                    projection.reset(&tx)?;
                    store::set_state(&tx, CURSOR, "0")?;
                    tx.commit()?;
                    report.reset = true;
                    continue;
                }
                other => other?,
            };
            {
                let mut c = self.connection();
                let tx = c.transaction()?;
                for e in &feed.changes {
                    self.apply(&tx, projection, e, &mut report)?;
                }
                store::set_state(&tx, CURSOR, &feed.next)?;
                tx.commit()?;
            }
            report.cursor = feed.next;
            if !feed.more {
                return Ok(report);
            }
        }
    }

    fn apply(
        &self,
        tx: &rusqlite::Transaction,
        projection: &mut dyn Projection,
        e: &Entry,
        report: &mut SyncReport,
    ) -> Result<()> {
        match (e.kind.as_str(), e.action) {
            ("object", Action::Delete) => {
                if let Some(o) = store::object(tx, &e.id)? {
                    projection.object(tx, &o, Action::Delete)?;
                }
                store::remove_object(tx, &e.id)?;
                report.objects += 1;
            }
            ("object", action) => {
                if let Some(o) = &e.object {
                    store::put_object(tx, o)?;
                    projection.object(tx, o, action)?;
                    report.objects += 1;
                }
            }
            ("link", Action::Delete) => {
                if let Ok(id) = e.id.parse::<i64>() {
                    store::remove_link(tx, id)?;
                    report.links += 1;
                }
            }
            ("link", action) => {
                if let Some(l) = &e.link {
                    store::put_link(tx, l)?;
                    projection.link(tx, l, action)?;
                    report.links += 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn object(&self, id: &str) -> Result<Option<ObjectRow>> {
        Ok(store::object(&self.connection(), id)?)
    }

    /// Live objects of a type, by title.
    pub fn objects(&self, type_key: &str) -> Result<Vec<ObjectRow>> {
        Ok(store::objects(&self.connection(), type_key)?)
    }

    /// Open links from an object, of one type or all.
    pub fn links_from(&self, id: &str, link_type: Option<&str>) -> Result<Vec<LinkRow>> {
        Ok(store::links_from(&self.connection(), id, link_type)?)
    }

    /// The local file of a media (`asset_id` = its library id, the
    /// `native_id` of its `media_asset` object) and its content type, to
    /// serve it without PneX.
    pub fn media(&self, asset_id: &str) -> Result<Option<(PathBuf, String)>> {
        let row: Option<(Option<String>, Option<String>)> = self
            .connection()
            .query_row(
                "SELECT sha256, content_type FROM media WHERE asset_id = ?1 AND fetched_at IS NOT NULL",
                [asset_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        Ok(match row {
            Some((Some(sha), ct)) => {
                let p = self.path_of(&sha);
                p.exists()
                    .then(|| (p, ct.unwrap_or_else(|| "application/octet-stream".into())))
            }
            _ => None,
        })
    }

    fn path_of(&self, sha: &str) -> PathBuf {
        self.config
            .media_dir
            .join(&sha[..2.min(sha.len())])
            .join(&sha[2.min(sha.len())..4.min(sha.len())])
            .join(sha)
    }

    /// Downloads or revalidates every media of the copy that the
    /// configuration wants. A failure leaves the last valid file.
    pub async fn fetch_media(&self) -> Result<MediaReport> {
        let mut report = MediaReport::default();
        let assets: Vec<(String, Option<String>)> = {
            let c = self.connection();
            let mut st = c.prepare("SELECT asset_id, etag FROM media ORDER BY asset_id")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (asset, etag) in assets {
            match self.fetch_one(&asset, etag.as_deref()).await {
                Ok(Fetch::Downloaded) => report.downloaded += 1,
                Ok(Fetch::Unchanged) => report.unchanged += 1,
                Ok(Fetch::Skipped) => report.skipped += 1,
                Err(e) => {
                    tracing::warn!(asset = %asset, error = %e, "media not copied");
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    async fn fetch_one(&self, asset: &str, etag: Option<&str>) -> Result<Fetch> {
        #[derive(Deserialize)]
        struct Detail {
            kind: String,
            #[serde(default)]
            size_bytes: Option<i64>,
            #[serde(default)]
            content_type: Option<String>,
        }
        let d: Detail = self
            .client
            .get_json(&format!("/api/v1/media/{asset}"))
            .await?;
        let size_ok = match (self.config.media_max_bytes, d.size_bytes) {
            (Some(max), Some(size)) => size >= 0 && size as u64 <= max,
            _ => true,
        };
        let wanted = self.config.media_kinds.contains(&d.kind) && size_ok;
        self.connection().execute(
            "UPDATE media SET kind = ?2, size_bytes = ?3, wanted = ?4 WHERE asset_id = ?1",
            rusqlite::params![asset, d.kind, d.size_bytes, wanted],
        )?;
        if !wanted {
            return Ok(Fetch::Skipped);
        }
        let content = format!("/api/v1/media/{asset}/content");
        let (bytes, new_etag) = match self.client.get(&content, etag).await? {
            Fetched::NotModified => return Ok(Fetch::Unchanged),
            Fetched::Body { bytes, etag } => (bytes, etag),
        };
        let (version, sha) = self.current_version(asset).await?;
        let got = hex(&Sha256::digest(&bytes));
        if sha.as_deref().is_some_and(|s| s != got) {
            return Err(Error::Checksum {
                asset_id: asset.to_string(),
            });
        }
        let path = self.path_of(&got);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            // Written aside then renamed: a cut leaves no half file.
            let tmp = path.with_extension("part");
            tokio::fs::write(&tmp, &bytes).await?;
            tokio::fs::rename(&tmp, &path).await?;
        }
        self.connection().execute(
            "UPDATE media SET version = ?2, sha256 = ?3, content_type = ?4, etag = ?5, \
             fetched_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE asset_id = ?1",
            rusqlite::params![asset, version, got, d.content_type, new_etag],
        )?;
        Ok(Fetch::Downloaded)
    }

    /// Number and SHA-256 of the media's current version.
    async fn current_version(&self, asset: &str) -> Result<(Option<i64>, Option<String>)> {
        #[derive(Deserialize)]
        struct Version {
            version_number: i64,
            #[serde(default)]
            sha256: Option<String>,
            #[serde(default)]
            current: bool,
        }
        #[derive(Deserialize)]
        struct Page {
            results: Vec<Version>,
        }
        let page: Page = self
            .client
            .get_json(&format!("/api/v1/media/{asset}/versions?limit=100"))
            .await?;
        Ok(page
            .results
            .into_iter()
            .find(|v| v.current)
            .map(|v| (Some(v.version_number), v.sha256))
            .unwrap_or((None, None)))
    }
}

enum Fetch {
    Downloaded,
    Unchanged,
    Skipped,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_of_a_digest() {
        use sha2::Digest;
        assert_eq!(
            super::hex(&sha2::Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
