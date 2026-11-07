//! Merge requests of coder agents (D388, D707): once the push sidecar of a run sent its
//! `pnex/run-<id>` branch, the server opens the merge request with the run's write token.
//! Merging stays a human act. V1 speaks GitHub only (D752); GitLab comes through the same
//! door. Every call goes to an address the egress rule of the org allows (R8).

use std::time::Duration;

use pnex_core::err_codes;
use reqwest::Client;
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);
const GITHUB_API: &str = "api.github.com";

/// Why a merge request was not opened: codes only, the outside server's text stays in logs.
pub type Refusal = &'static str;

/// `(owner, repository)` of an `https://github.com/<owner>/<repo>[.git]` URL.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// The branch a run pushes: never the base, never a name an agent chose.
pub fn run_branch(run_id: &str) -> String {
    format!("pnex/run-{run_id}")
}

async fn client() -> Result<Client, Refusal> {
    let policy = pnex_core::egress::policy();
    if pnex_core::egress::check_host(GITHUB_API, policy).is_some() {
        return Err(err_codes::AGENT_RUN_FORGE_REFUSED);
    }
    let addrs = tokio::net::lookup_host((GITHUB_API, 443))
        .await
        .map_err(|_| err_codes::AGENT_RUN_MERGE_REQUEST_FAILED)?;
    let Some(addr) = pnex_core::egress::filter_addrs(addrs, policy)
        .into_iter()
        .next()
    else {
        return Err(err_codes::AGENT_RUN_FORGE_REFUSED);
    };
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .resolve(GITHUB_API, addr)
        .user_agent("pnex-server")
        .build()
        .map_err(|_| err_codes::AGENT_RUN_MERGE_REQUEST_FAILED)
}

/// Opens the merge request of `branch` onto `base`, or finds the one already open (a
/// retried job end opens none twice). `Ok(None)`: the branch was never pushed (no change,
/// or the push failed), nothing to open.
pub async fn open_merge_request(
    repo_url: &str,
    base: &str,
    branch: &str,
    token: &str,
    title: &str,
    body: &str,
) -> Result<Option<String>, Refusal> {
    let (owner, repo) = github_repo(repo_url).ok_or(err_codes::AGENT_RUN_FORGE_UNSUPPORTED)?;
    let http = client().await?;
    let api = format!("https://{GITHUB_API}/repos/{owner}/{repo}");
    let get = |url: String| {
        http.get(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .send()
    };
    let failed = |e: reqwest::Error| {
        tracing::info!(error = %e.without_url(), "forge not reached");
        err_codes::AGENT_RUN_MERGE_REQUEST_FAILED
    };
    let branch_ref = get(format!("{api}/git/ref/heads/{branch}"))
        .await
        .map_err(failed)?;
    if branch_ref.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let open: Value = get(format!(
        "{api}/pulls?state=open&head={owner}:{branch}&base={base}"
    ))
    .await
    .map_err(failed)?
    .json()
    .await
    .map_err(failed)?;
    if let Some(url) = open[0]["html_url"].as_str() {
        return Ok(Some(url.to_string()));
    }
    let created = http
        .post(format!("{api}/pulls"))
        .bearer_auth(token)
        .header("accept", "application/vnd.github+json")
        .json(&json!({ "title": title, "head": branch, "base": base, "body": body }))
        .send()
        .await
        .map_err(failed)?;
    if !created.status().is_success() {
        tracing::info!(status = %created.status(), "merge request refused by the forge");
        return Err(err_codes::AGENT_RUN_MERGE_REQUEST_FAILED);
    }
    let created: Value = created.json().await.map_err(failed)?;
    Ok(created["html_url"].as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_github_repositories_are_understood() {
        assert_eq!(
            github_repo("https://github.com/acme/demo.git"),
            Some(("acme".into(), "demo".into()))
        );
        assert_eq!(
            github_repo("https://github.com/acme/demo/"),
            Some(("acme".into(), "demo".into()))
        );
        for bad in [
            "https://gitlab.com/acme/demo",
            "https://github.com/acme",
            "https://github.com/acme/demo/extra",
            "https://github.com/../demo",
            "https://github.com/acme/de?mo",
            "http://github.com/acme/demo",
        ] {
            assert_eq!(github_repo(bad), None, "{bad}");
        }
        assert_eq!(run_branch("ab12"), "pnex/run-ab12");
    }
}
