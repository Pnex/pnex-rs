//! `source_resolve` (D317): a fixed URL, or the most recent resource of a
//! data.gouv.fr dataset whose title matches a glob (the file name changes
//! every day). Behind the egress guard (R8).

use pnex_core::err_codes;
use pnex_core::ingest::{glob_match, IngestConfig, IngestMsg, ResolveMode};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Outcome;

/// data.gouv.fr API root; `PNEX_DATAGOUV_API_URL` overrides it (tests, a
/// mirror).
fn api_root() -> String {
    std::env::var("PNEX_DATAGOUV_API_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://www.data.gouv.fr/api/1".into())
}

#[derive(Debug, Deserialize)]
struct Dataset {
    #[serde(default)]
    title: String,
    #[serde(default)]
    resources: Vec<Resource>,
}

#[derive(Debug, Clone, Deserialize)]
struct Resource {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    filesize: Option<Value>,
    #[serde(default)]
    last_modified: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl Resource {
    fn date(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        let raw = self
            .last_modified
            .as_deref()
            .or(self.created_at.as_deref())?;
        chrono::DateTime::parse_from_rfc3339(raw).ok().or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|t| t.and_utc().fixed_offset())
        })
    }
}

/// The matching resource with the latest date (then the latest title).
fn newest<'a>(resources: &'a [Resource], pattern: &str) -> Option<&'a Resource> {
    resources
        .iter()
        .filter(|r| glob_match(pattern, &r.title) || glob_match(pattern, &r.url))
        .max_by(|a, b| a.date().cmp(&b.date()).then_with(|| a.title.cmp(&b.title)))
}

pub(super) async fn run(cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let unreachable = || Outcome::fail(err_codes::IMPORT_SOURCE_UNREACHABLE, Value::Null);
    let check = crate::services::ontology::imports::source::check_url;
    if cfg.mode == ResolveMode::Url {
        let Ok(u) = check(&cfg.url) else {
            return unreachable();
        };
        return Outcome::ok(
            IngestMsg {
                run_id: msg.run_id,
                url: u.to_string(),
                ..Default::default()
            },
            json!({ "url": u.as_str() }),
        );
    }
    let dataset: String = cfg
        .dataset
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let Ok(api) = check(&format!("{}/datasets/{dataset}/", api_root())) else {
        return unreachable();
    };
    let doc: Dataset = match super::download::http().get(api).send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(d) => d,
            Err(_) => return Outcome::fail(err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null),
        },
        _ => return unreachable(),
    };
    let pattern = cfg.resource_match.trim();
    let Some(r) = newest(&doc.resources, pattern) else {
        return Outcome::fail(err_codes::INGEST_NO_RESOURCE, json!({ "pattern": pattern }));
    };
    let Ok(u) = check(&r.url) else {
        return unreachable();
    };
    Outcome::ok(
        IngestMsg {
            run_id: msg.run_id,
            url: u.to_string(),
            ..Default::default()
        },
        json!({
            "url": u.as_str(),
            "title": r.title,
            "format": r.format,
            "size": r.filesize,
            "last_modified": r.last_modified.as_ref().or(r.created_at.as_ref()),
            "dataset_title": doc.title,
            "candidates": doc.resources.iter().filter(|x| glob_match(pattern, &x.title)).count(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(title: &str, at: &str) -> Resource {
        Resource {
            title: title.into(),
            url: format!("https://static.data.gouv.fr/{title}"),
            format: Some("zip".into()),
            filesize: None,
            last_modified: Some(at.into()),
            created_at: None,
        }
    }

    #[test]
    fn newest_matching_resource_wins() {
        let rs = vec![
            res(
                "export-fiches-csv-2026-10-19.zip",
                "2026-10-19T03:00:00+00:00",
            ),
            res(
                "export-fiches-csv-2026-10-20.zip",
                "2026-10-20T03:00:00.123000",
            ),
            res(
                "export-fiches-xml-2026-10-21.zip",
                "2026-10-21T03:00:00+00:00",
            ),
        ];
        let r = newest(&rs, "export-fiches-csv-*.zip").unwrap();
        assert_eq!(r.title, "export-fiches-csv-2026-10-20.zip");
        assert!(newest(&rs, "*.json").is_none());
    }
}
