//! Layer cache of image builds on S3 (agent-environments.md D701, D730). A build pod runs
//! a Dockerfile and can read what it holds, so it never gets the keys of this server:
//! it gets temporary credentials, limited by a session policy to one prefix of one
//! environment. A store without STS simply means builds without a cache.

use std::time::Duration;

use pnex_core::runners::RunnerBuildCache;
use sha2::{Digest, Sha256};

use crate::services::artifact_store::S3Config;

const TIMEOUT: Duration = Duration::from_secs(15);
/// Multipart uploads included. No listing: a key is guessable only by who wrote it, and
/// BuildKit addresses its objects by name (measured without it, 2026-11-05).
const ACTIONS: [&str; 4] = [
    "s3:GetObject",
    "s3:PutObject",
    "s3:AbortMultipartUpload",
    "s3:ListMultipartUploadParts",
];

/// Who signs a request.
pub struct SigningKeys<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub session_token: Option<&'a str>,
}

/// One S3 prefix: its environment's layers, and nothing else of the org.
pub fn prefix_of(org_id: i64, environment: uuid::Uuid) -> String {
    format!("org_{org_id}/buildcache/{environment}/")
}

fn session_policy(bucket: &str, prefix: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ACTIONS,
            "Resource": format!("arn:aws:s3:::{bucket}/{prefix}*"),
        }]
    })
    .to_string()
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new()
        .chain_update(block.map(|b| b ^ 0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(block.map(|b| b ^ 0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac_sha256(&k, region.as_bytes());
    let k = hmac_sha256(&k, service.as_bytes());
    hmac_sha256(&k, b"aws4_request")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A request signed with AWS Signature V4. `url` is already percent-encoded.
pub fn signed(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &reqwest::Url,
    (service, region): (&str, &str),
    keys: &SigningKeys<'_>,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> reqwest::RequestBuilder {
    let now = chrono::Utc::now();
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let payload = hex(&Sha256::digest(&body));
    // Sorted by name, as the signature requires.
    let mut headers = vec![
        ("host", host),
        ("x-amz-content-sha256", payload.clone()),
        ("x-amz-date", stamp.clone()),
    ];
    if let Some(kind) = content_type {
        headers.push(("content-type", kind.to_string()));
    }
    if let Some(token) = keys.session_token {
        headers.push(("x-amz-security-token", token.to_string()));
    }
    headers.sort();
    let canonical: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let names = headers
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(";");
    let request = format!(
        "{method}\n{}\n{}\n{canonical}\n{names}\n{payload}",
        url.path(),
        url.query().unwrap_or_default(),
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex(&Sha256::digest(request.as_bytes()))
    );
    let signature = hmac_sha256(
        &signing_key(keys.secret_key, &date, region, service),
        to_sign.as_bytes(),
    );
    let mut out = client.request(method, url.clone()).header(
        "authorization",
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={}",
            keys.access_key,
            hex(&signature)
        ),
    );
    for (name, value) in headers.iter().filter(|(k, _)| *k != "host") {
        out = out.header(*name, value);
    }
    out.body(body)
}

fn between<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

/// Temporary credentials for this prefix, valid `seconds`. `endpoint` is how a build pod
/// reaches the store, which may not be how this server does. `None` when the store has
/// no STS or refuses: the build runs without a cache, the reason stays in the logs.
pub async fn for_prefix(
    config: &S3Config,
    endpoint: &str,
    prefix: &str,
    seconds: u32,
) -> Option<RunnerBuildCache> {
    let region = if config.region.is_empty() {
        "us-east-1"
    } else {
        &config.region
    };
    let duration = seconds.to_string();
    let policy = session_policy(&config.bucket, prefix);
    let form = reqwest::Url::parse_with_params(
        "http://form/",
        [
            ("Action", "AssumeRole"),
            ("Version", "2011-06-15"),
            ("RoleArn", "arn:aws:iam:::role/pnex-build-cache"),
            ("RoleSessionName", "pnex-build-cache"),
            ("DurationSeconds", duration.as_str()),
            ("Policy", policy.as_str()),
        ],
    )
    .ok()?;
    let url = reqwest::Url::parse(&format!("{}/", config.endpoint.trim_end_matches('/'))).ok()?;
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let keys = SigningKeys {
        access_key: &config.access_key,
        secret_key: &config.secret_key,
        session_token: None,
    };
    let answer = signed(
        &client,
        reqwest::Method::POST,
        &url,
        ("sts", region),
        &keys,
        Some("application/x-www-form-urlencoded"),
        form.query().unwrap_or_default().as_bytes().to_vec(),
    )
    .send()
    .await;
    let answer = match answer {
        Ok(a) if a.status().is_success() => a,
        Ok(a) => {
            tracing::warn!(status = %a.status(), "no build cache: the store refused STS");
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e.without_url(), "no build cache: store not reached");
            return None;
        }
    };
    let xml = answer.text().await.ok()?;
    Some(RunnerBuildCache {
        endpoint: endpoint.to_string(),
        region: region.to_string(),
        bucket: config.bucket.clone(),
        prefix: prefix.to_string(),
        access_key: between(&xml, "AccessKeyId")?.to_string(),
        secret_key: between(&xml, "SecretAccessKey")?.to_string(),
        session_token: between(&xml, "SessionToken")?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example of the AWS documentation.
    #[test]
    fn the_signing_key_matches_the_published_vector() {
        let key = signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn the_policy_names_one_prefix_and_no_listing() {
        let policy = session_policy("b", "org_1/buildcache/e/");
        assert!(
            policy.contains("arn:aws:s3:::b/org_1/buildcache/e/*"),
            "{policy}"
        );
        assert!(!policy.contains("ListBucket\""), "{policy}");
        assert!(!policy.contains("s3:*"), "{policy}");
    }

    #[test]
    fn a_tag_is_read_from_the_answer() {
        let xml = "<a><AccessKeyId>K</AccessKeyId><SessionToken>T+/=</SessionToken></a>";
        assert_eq!(between(xml, "AccessKeyId"), Some("K"));
        assert_eq!(between(xml, "SessionToken"), Some("T+/="));
        assert_eq!(between(xml, "Other"), None);
    }
}
