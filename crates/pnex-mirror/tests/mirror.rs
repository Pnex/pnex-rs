//! The mirror against a fake PneX on an invented ontology (`article`,
//! `auteur`, `ecrit_par`): pages applied with their cursor, a resume, an
//! expired cursor, media by kind with a 304 revalidation and a wrong
//! checksum refused, and a projection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pnex_mirror::{Action, Config, Mirror, NoProjection, ObjectRow, Projection};
use serde_json::{json, Value};
use sha2::Digest;

const KEY: &str = "pnex_sk_test";

#[derive(Default)]
struct Fake {
    /// Answer 410 once to a cursor other than 0.
    expired: bool,
    /// Cursor 1.6 has a new page (an update of the article).
    later: bool,
    content_hits: usize,
    not_modified: usize,
}

type Shared = Arc<Mutex<Fake>>;

fn sha(b: &[u8]) -> String {
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn object(id: &str, t: &str, title: &str, version: i64, native: Option<&str>) -> Value {
    json!({"id": id, "type_key": t, "title": title, "native_id": native.unwrap_or(id), "system": false,
           "properties": {"lang": "fr"}, "version": version, "valid_from": "2026-10-25T10:00:00Z",
           "created_at": "2026-10-25T10:00:00Z", "updated_at": "2026-10-25T10:00:00Z", "source_ref": "import:1@1"})
}

fn entry(cursor: &str, kind: &str, action: &str, id: &str, t: &str, body: Value) -> Value {
    let mut e = json!({"cursor": cursor, "at": "2026-10-25T10:00:00Z", "kind": kind, "action": action, "id": id, "type_key": t});
    e[kind] = body;
    e
}

fn authorized(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {KEY}"))
}

async fn changes(
    State(s): State<Shared>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !authorized(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let since = q.get("since").cloned().unwrap_or_default();
    let mut f = s.lock().unwrap();
    if f.expired && since != "0" {
        f.expired = false;
        return (
            StatusCode::GONE,
            Json(json!({"error": "changes-cursor-expired"})),
        )
            .into_response();
    }
    let link = json!({"id": 7, "link_type": "ecrit_par", "source": {"id": "a1", "type_key": "article", "title": "Moteurs"},
                      "target": {"id": "b1", "type_key": "auteur", "title": "Ada"}, "attributes": {}, "valid_from": "2026-10-25T10:00:00Z"});
    let page = match since.as_str() {
        "0" => json!({"changes": [
            entry("1.1", "object", "create", "a1", "article", object("a1", "article", "Moteurs", 1, None)),
            entry("1.2", "object", "create", "b1", "auteur", object("b1", "auteur", "Ada", 1, None)),
        ], "next": "1.2", "more": true}),
        "1.2" => json!({"changes": [
            entry("1.3", "object", "create", "m1", "media_asset", object("m1", "media_asset", "capture", 1, Some("asset-1"))),
            entry("1.4", "object", "create", "m2", "media_asset", object("m2", "media_asset", "faux", 1, Some("asset-2"))),
            entry("1.5", "link", "create", "7", "ecrit_par", link),
        ], "next": "1.6", "more": false}),
        "1.6" if f.later => json!({"changes": [
            entry("2.1", "object", "update", "a1", "article", object("a1", "article", "Moteurs 2", 2, None)),
            entry("2.2", "object", "delete", "b1", "auteur", Value::Null),
        ], "next": "2.2", "more": false}),
        other => json!({"changes": [], "next": other, "more": false}),
    };
    Json(page).into_response()
}

async fn media(Path(id): Path<String>) -> Json<Value> {
    Json(
        json!({"id": id, "kind": "photo", "name": id, "size_bytes": 3, "content_type": "image/png"}),
    )
}

async fn versions(Path(id): Path<String>) -> Json<Value> {
    // asset-2 announces a hash its content does not have.
    let s = if id == "asset-2" {
        sha(b"other")
    } else {
        sha(b"abc")
    };
    Json(json!({"count": 1, "next": null, "previous": null,
                "results": [{"version_number": 1, "sha256": s, "current": true}]}))
}

async fn content(State(s): State<Shared>, h: HeaderMap) -> Response {
    let mut f = s.lock().unwrap();
    f.content_hits += 1;
    if h.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"v1\"") {
        f.not_modified += 1;
        return StatusCode::NOT_MODIFIED.into_response();
    }
    (
        [("etag", "\"v1\""), ("content-type", "image/png")],
        b"abc".to_vec(),
    )
        .into_response()
}

async fn me() -> Json<Value> {
    Json(
        json!({"key": "reader", "scope": {"types": ["article", "auteur", "media_asset"], "link_types": ["ecrit_par"],
                                         "media_kinds": ["photo"], "media_max_bytes": 1000}}),
    )
}

async fn serve() -> (String, Shared) {
    let state: Shared = Arc::default();
    let app = Router::new()
        .route("/api/v1/ontology/changes", get(changes))
        .route("/api/v1/apps/me", get(me))
        .route("/api/v1/media/{id}", get(media))
        .route("/api/v1/media/{id}/versions", get(versions))
        .route("/api/v1/media/{id}/content", get(content))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pnex-mirror-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Counts what it saw and keeps titles in its own table.
#[derive(Default)]
struct Titles {
    seen: Vec<(String, Action)>,
    resets: usize,
}

impl Projection for Titles {
    fn object(
        &mut self,
        tx: &rusqlite::Transaction,
        o: &ObjectRow,
        action: Action,
    ) -> rusqlite::Result<()> {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS titles (id TEXT PRIMARY KEY, title TEXT)",
            [],
        )?;
        if action == Action::Delete {
            tx.execute("DELETE FROM titles WHERE id = ?1", [&o.id])?;
        } else {
            tx.execute(
                "INSERT INTO titles (id, title) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET title = excluded.title",
                [&o.id, &o.title],
            )?;
        }
        self.seen.push((o.id.clone(), action));
        Ok(())
    }
    fn reset(&mut self, _tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        self.resets += 1;
        Ok(())
    }
}

#[tokio::test]
async fn a_copy_follows_the_feed_and_keeps_media_by_hash() {
    let (base, state) = serve().await;
    let d = dir("copy");
    let config = Config::new(&base, KEY, &d).from_app().await.unwrap();
    assert_eq!(config.media_kinds, vec!["photo"]);
    let mirror = Mirror::open(config.clone()).unwrap();
    let mut titles = Titles::default();

    let r = mirror.sync(&mut titles).await.unwrap();
    assert_eq!(
        (r.objects, r.links, r.reset, r.cursor.as_str()),
        (4, 1, false, "1.6")
    );
    assert_eq!(mirror.cursor().unwrap(), "1.6");
    assert_eq!(
        mirror.objects("article").unwrap()[0].properties["lang"],
        "fr"
    );
    assert_eq!(
        mirror.links_from("a1", Some("ecrit_par")).unwrap()[0]
            .target
            .id,
        "b1"
    );
    assert_eq!(titles.seen.len(), 4);

    // Media: the good one stored by hash, the lying one refused.
    let m = mirror.fetch_media().await.unwrap();
    assert_eq!((m.downloaded, m.failed), (1, 1), "{m:?}");
    let (path, ct) = mirror.media("asset-1").unwrap().expect("local file");
    assert!(path.ends_with(sha(b"abc")), "{path:?}");
    assert_eq!(
        (std::fs::read(&path).unwrap(), ct.as_str()),
        (b"abc".to_vec(), "image/png")
    );
    assert!(mirror.media("asset-2").unwrap().is_none());
    // Revalidated, not downloaded again.
    let m = mirror.fetch_media().await.unwrap();
    assert_eq!(m.unchanged, 1, "{m:?}");
    assert!(state.lock().unwrap().not_modified >= 1);

    // Reopened: it resumes from its cursor, then sees what came after.
    drop(mirror);
    let mirror = Mirror::open(config.clone()).unwrap();
    assert_eq!(
        mirror.sync(&mut NoProjection).await.unwrap().objects,
        0,
        "nothing new"
    );
    state.lock().unwrap().later = true;
    let mut titles = Titles::default();
    let r = mirror.sync(&mut titles).await.unwrap();
    assert_eq!((r.objects, r.cursor.as_str()), (2, "2.2"));
    assert_eq!(mirror.object("a1").unwrap().unwrap().title, "Moteurs 2");
    assert!(mirror.object("b1").unwrap().is_none(), "deleted");
    assert_eq!(
        titles.seen,
        vec![
            ("a1".to_string(), Action::Update),
            ("b1".to_string(), Action::Delete)
        ]
    );
    let projected: i64 = mirror
        .connection()
        .query_row("SELECT count(*) FROM titles", [], |r| r.get(0))
        .unwrap_or(0);
    assert_eq!(
        projected, 3,
        "the projection is in the mirror's transaction (a1, m1, m2)"
    );

    // An expired cursor empties the copy and starts again from 0.
    {
        let mut f = state.lock().unwrap();
        f.expired = true;
        f.later = false;
    }
    let mut titles = Titles::default();
    let r = mirror.sync(&mut titles).await.unwrap();
    assert!(r.reset);
    assert_eq!(titles.resets, 1);
    assert_eq!(mirror.objects("auteur").unwrap().len(), 1, "copied again");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn a_wrong_key_is_refused_at_once() {
    let (base, _) = serve().await;
    let d = dir("refused");
    let mirror = Mirror::open(Config::new(&base, "pnex_sk_wrong", &d)).unwrap();
    let e = mirror.sync(&mut NoProjection).await.unwrap_err();
    assert_eq!(e.status(), Some(401));
    let _ = std::fs::remove_dir_all(&d);
}
