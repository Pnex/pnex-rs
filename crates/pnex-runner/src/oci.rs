//! An image pinned by digest, unpacked into a folder (agent-environments.md D762): the root
//! file system of a micro-VM. Every blob is checked against its digest; a layer never
//! writes outside the folder, and its whiteouts remove what an earlier layer added.

use std::io::Read;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

const MANIFEST_TYPES: &str = "application/vnd.oci.image.index.v1+json, \
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.docker.distribution.manifest.v2+json";

/// `host/repo/path@sha256:<hex>` split in `(host, repository, digest)`.
pub fn parse_reference(image: &str) -> Option<(String, String, String)> {
    let (name, digest) = image.split_once('@')?;
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (host, repo) = name.split_once('/')?;
    if !(host.contains('.') || host.contains(':') || host == "localhost") || repo.is_empty() {
        return None;
    }
    let repo = repo.split(':').next()?.to_string();
    Some((host.to_string(), repo, digest.to_string()))
}

fn sha256_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub struct Registry {
    http: reqwest::Client,
    base: String,
    repo: String,
    token: Option<String>,
    login: Option<(String, String)>,
}

impl Registry {
    /// `insecure`: plain HTTP, for a registry on the runner's own network only.
    pub fn new(host: &str, repo: &str, insecure: bool, login: Option<(String, String)>) -> Self {
        let scheme = if insecure { "http" } else { "https" };
        Self {
            http: reqwest::Client::new(),
            base: format!("{scheme}://{host}/v2/{repo}"),
            repo: repo.to_string(),
            token: None,
            login,
        }
    }

    /// The bearer token a registry asks for in its `WWW-Authenticate` challenge.
    async fn authenticate(&mut self, challenge: &str) -> Result<(), String> {
        let field = |key: &str| {
            challenge
                .split(&format!("{key}=\""))
                .nth(1)
                .and_then(|r| r.split('"').next())
                .map(str::to_string)
        };
        let realm = field("realm").ok_or("registry challenge without realm")?;
        let mut req = self.http.get(&realm).query(&[
            ("service", field("service").unwrap_or_default()),
            ("scope", format!("repository:{}:pull", self.repo)),
        ]);
        if let Some((user, secret)) = &self.login {
            req = req.basic_auth(user, Some(secret));
        }
        let body: Value = req
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?
            .error_for_status()
            .map_err(|e| e.without_url().to_string())?
            .json()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let token = body["token"]
            .as_str()
            .or_else(|| body["access_token"].as_str())
            .ok_or("registry token missing")?;
        self.token = Some(token.to_string());
        Ok(())
    }

    async fn get(&mut self, path: &str, accept: &str) -> Result<Vec<u8>, String> {
        for _ in 0..2 {
            let mut req = self
                .http
                .get(format!("{}/{path}", self.base))
                .header("Accept", accept);
            if let Some(t) = &self.token {
                req = req.bearer_auth(t);
            }
            let res = req.send().await.map_err(|e| e.without_url().to_string())?;
            if res.status() == reqwest::StatusCode::UNAUTHORIZED && self.token.is_none() {
                let challenge = res
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                self.authenticate(&challenge).await?;
                continue;
            }
            let res = res
                .error_for_status()
                .map_err(|e| e.without_url().to_string())?;
            return Ok(res
                .bytes()
                .await
                .map_err(|e| e.without_url().to_string())?
                .to_vec());
        }
        Err(format!("{path}: unauthorized"))
    }

    /// A blob, refused unless it hashes to its digest.
    async fn blob(&mut self, digest: &str, accept: &str, path: &str) -> Result<Vec<u8>, String> {
        let bytes = self.get(path, accept).await?;
        if sha256_of(&bytes) != digest {
            return Err(format!("{digest}: content does not match its digest"));
        }
        Ok(bytes)
    }

    /// Digests of the layers of the image, in order: the linux/amd64 one of an index.
    pub async fn layers(&mut self, digest: &str) -> Result<Vec<String>, String> {
        let raw = self
            .blob(digest, MANIFEST_TYPES, &format!("manifests/{digest}"))
            .await?;
        let mut manifest: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        if let Some(list) = manifest["manifests"].as_array() {
            let pick = list
                .iter()
                .find(|m| {
                    m["platform"]["os"] == "linux"
                        && m["platform"]["architecture"]
                            == std::env::consts::ARCH.replace("x86_64", "amd64")
                })
                .and_then(|m| m["digest"].as_str())
                .ok_or("no manifest for this platform")?
                .to_string();
            let raw = self
                .blob(&pick, MANIFEST_TYPES, &format!("manifests/{pick}"))
                .await?;
            manifest = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        }
        Ok(manifest["layers"]
            .as_array()
            .ok_or("manifest without layers")?
            .iter()
            .filter_map(|l| l["digest"].as_str().map(str::to_string))
            .collect())
    }

    pub async fn layer(&mut self, digest: &str) -> Result<Vec<u8>, String> {
        self.blob(digest, "*/*", &format!("blobs/{digest}")).await
    }
}

/// Unpacks one layer (gzip or plain tar) into `root`, applying its whiteouts.
pub fn apply_layer(root: &Path, layer: &[u8]) -> Result<(), String> {
    let reader: Box<dyn Read> = if layer.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(layer))
    } else {
        Box::new(layer)
    };
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let parent = path.parent().unwrap_or(Path::new(""));
        if name == ".wh..wh..opq" {
            // Opaque folder: what lower layers put there goes.
            let dir = safe_join(root, parent)?;
            if let Ok(read) = std::fs::read_dir(&dir) {
                for child in read.flatten() {
                    remove(&child.path());
                }
            }
            continue;
        }
        if let Some(gone) = name.strip_prefix(".wh.") {
            remove(&safe_join(root, &parent.join(gone))?);
            continue;
        }
        // `unpack_in` refuses any path that would leave `root`.
        if !entry.unpack_in(root).map_err(|e| e.to_string())? {
            return Err(format!("{}: outside the root, refused", path.display()));
        }
        readable_by_owner(&safe_join(root, &path)?);
    }
    Ok(())
}

/// The runner reads every file to make the disk (`/etc/shadow` is often `0000`): the owner
/// gets read, and folders their search bit. Inside the VM, root reads them all anyway.
pub(crate) fn readable_by_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mode = meta.permissions().mode();
    let wanted = if meta.is_dir() {
        mode | 0o700
    } else {
        mode | 0o600
    };
    if wanted != mode {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(wanted));
    }
}

fn remove(path: &Path) {
    let _ = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => Ok(()),
    };
}

/// `root/rel`, refused when `rel` climbs out.
pub(crate) fn safe_join(root: &Path, rel: &Path) -> Result<std::path::PathBuf, String> {
    use std::path::Component;
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("{}: outside the root, refused", rel.display()));
    }
    Ok(root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, body) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, path, *body).unwrap();
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn references_are_pinned_and_split() {
        let d = "a".repeat(64);
        assert_eq!(
            parse_reference(&format!("kind-registry:5000/agents/demo@sha256:{d}")),
            Some((
                "kind-registry:5000".into(),
                "agents/demo".into(),
                format!("sha256:{d}")
            ))
        );
        assert_eq!(
            parse_reference("cgr.dev/x/y:latest"),
            None,
            "a tag is never enough"
        );
        assert_eq!(
            parse_reference(&format!("x@sha256:{d}")),
            None,
            "a host is required"
        );
    }

    #[test]
    fn layers_stack_with_their_whiteouts_and_never_leave_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        apply_layer(
            root,
            &tar_of(&[("etc/a", b"1"), ("etc/b", b"2"), ("opt/x/y", b"3")]),
        )
        .unwrap();
        // Gzip of the second layer, as registries serve it.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(
            &mut gz,
            &tar_of(&[("etc/.wh.a", b""), ("opt/x/.wh..wh..opq", b"")]),
        )
        .unwrap();
        apply_layer(root, &gz.finish().unwrap()).unwrap();
        assert!(!root.join("etc/a").exists(), "whiteout removes");
        assert!(root.join("etc/b").exists());
        assert!(!root.join("opt/x/y").exists(), "opaque folder emptied");
        // A path climbing out is refused, nothing lands outside.
        let mut raw = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../escape.tx");
        h.set_cksum();
        raw.append(&h, &b"x"[..]).unwrap();
        let escaping = raw.into_inner().unwrap();
        assert!(apply_layer(&root.join("etc"), &escaping).is_err());
        assert!(!root.join("escape.tx").exists());
    }
}
