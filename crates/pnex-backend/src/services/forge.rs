//! Merge requests of coder agents (D388, D707): once the push sidecar of a run sent its
//! `pnex/run-<id>` branch, the server opens the merge request with the run's write token.
//! Merging stays a human act. GitHub at github.com, GitLab at any other host (D752). Every
//! call goes to the repository's own host, at an address the egress rule of the org
//! allows (R8).

use std::time::Duration;

use pnex_core::agent_runs::{forge_repo, Forge};
use pnex_core::err_codes;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);
const GITHUB_API: &str = "api.github.com";

/// Why a merge request was not opened: codes only, the outside server's text stays in logs.
pub type Refusal = &'static str;

pub use pnex_core::agent_runs::github_repo;

/// The branch a run pushes: never the base, never a name an agent chose.
pub fn run_branch(run_id: &str) -> String {
    format!("pnex/run-{run_id}")
}

async fn client(host: &str) -> Result<Client, Refusal> {
    let policy = pnex_core::egress::policy();
    if pnex_core::egress::check_host(host, policy).is_some() {
        return Err(err_codes::AGENT_RUN_FORGE_REFUSED);
    }
    let addrs = tokio::net::lookup_host((host, 443))
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
        .resolve(host, addr)
        .user_agent("pnex-server")
        .build()
        .map_err(|_| err_codes::AGENT_RUN_MERGE_REQUEST_FAILED)
}

fn failed(e: reqwest::Error) -> Refusal {
    tracing::info!(error = %e.without_url(), "forge not reached");
    err_codes::AGENT_RUN_MERGE_REQUEST_FAILED
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
    match forge_repo(repo_url).ok_or(err_codes::AGENT_RUN_FORGE_UNSUPPORTED)? {
        Forge::GitHub { owner, repo } => {
            github(&owner, &repo, base, branch, token, title, body).await
        }
        Forge::GitLab { host, project } => {
            gitlab(&host, &project, base, branch, token, title, body).await
        }
    }
}

async fn github(
    owner: &str,
    repo: &str,
    base: &str,
    branch: &str,
    token: &str,
    title: &str,
    body: &str,
) -> Result<Option<String>, Refusal> {
    let http = client(GITHUB_API).await?;
    let api = format!("https://{GITHUB_API}/repos/{owner}/{repo}");
    let get = |url: String| {
        http.get(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .send()
    };
    let branch_ref = get(format!("{api}/git/ref/heads/{branch}"))
        .await
        .map_err(failed)?;
    if branch_ref.status() == StatusCode::NOT_FOUND {
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

/// GitLab REST v4 base of a project: its path, `/` escaped, stands for its id.
fn gitlab_api(host: &str, project: &str) -> String {
    format!(
        "https://{host}/api/v4/projects/{}",
        project.replace('/', "%2F")
    )
}

async fn gitlab(
    host: &str,
    project: &str,
    base: &str,
    branch: &str,
    token: &str,
    title: &str,
    body: &str,
) -> Result<Option<String>, Refusal> {
    let http = client(host).await?;
    let api = gitlab_api(host, project);
    let branch_ref = http
        .get(format!(
            "{api}/repository/branches/{}",
            branch.replace('/', "%2F")
        ))
        .header("private-token", token)
        .send()
        .await
        .map_err(failed)?;
    if branch_ref.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let open: Value = http
        .get(format!("{api}/merge_requests"))
        .query(&[
            ("state", "opened"),
            ("source_branch", branch),
            ("target_branch", base),
        ])
        .header("private-token", token)
        .send()
        .await
        .map_err(failed)?
        .json()
        .await
        .map_err(failed)?;
    if let Some(url) = open[0]["web_url"].as_str() {
        return Ok(Some(url.to_string()));
    }
    let created = http
        .post(format!("{api}/merge_requests"))
        .header("private-token", token)
        .json(&json!({
            "source_branch": branch,
            "target_branch": base,
            "title": title,
            "description": body,
        }))
        .send()
        .await
        .map_err(failed)?;
    if !created.status().is_success() {
        tracing::info!(status = %created.status(), "merge request refused by the forge");
        return Err(err_codes::AGENT_RUN_MERGE_REQUEST_FAILED);
    }
    let created: Value = created.json().await.map_err(failed)?;
    Ok(created["web_url"].as_str().map(str::to_string))
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
        assert_eq!(
            gitlab_api("gitlab-dev.pnex.io", "demo/team/items-api"),
            "https://gitlab-dev.pnex.io/api/v4/projects/demo%2Fteam%2Fitems-api"
        );
    }
}
