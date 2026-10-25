//! Use case: keep an offline copy of what an application reads.
//!
//! Needs: an application declared in the org (Access › Applications) with
//! its service account, a role that reads (`viewer`) and a key.
//! Routes: `GET /api/v1/apps/me`, `GET /api/v1/ontology/changes`,
//! `GET /api/v1/media/{id}`, `…/versions`, `…/content`.
//!
//! ```sh
//! PNEX_URL=https://pnex.example PNEX_KEY=pnex_sk_… \
//!   cargo run -p pnex-mirror --example offline_copy -- ./copy
//! ```
//!
//! Every 15 minutes it applies what changed since its cursor, then
//! downloads or revalidates the media of its scope. Cut it at any time:
//! it resumes where it stopped. When PneX is down, the copy stays as it
//! was and the application keeps serving it ([`Mirror::media`] gives the
//! local file of a media).

use std::time::Duration;

use pnex_mirror::{Config, Mirror, NoProjection};

#[tokio::main]
async fn main() -> pnex_mirror::Result<()> {
    let url = std::env::var("PNEX_URL").expect("PNEX_URL");
    let key = std::env::var("PNEX_KEY").expect("PNEX_KEY");
    let dir = std::env::args().nth(1).unwrap_or_else(|| "./copy".into());
    let config = Config::new(&url, &key, &dir).from_app().await?;
    let mirror = Mirror::open(config)?;
    loop {
        match mirror.sync(&mut NoProjection).await {
            Ok(r) => println!(
                "{} objects, {} links, cursor {}",
                r.objects, r.links, r.cursor
            ),
            Err(e) => eprintln!("sync failed, the copy stays as it was: {e}"),
        }
        match mirror.fetch_media().await {
            Ok(m) => println!(
                "media: {} new, {} unchanged, {} failed",
                m.downloaded, m.unchanged, m.failed
            ),
            Err(e) => eprintln!("media pass failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(15 * 60)).await;
    }
}
