//! Artifacts of a run (agent-environments.md D729, D744): two containers of their own move
//! them through PneX, never the agent and never the runner. `artifacts-in` fetches each
//! input and checks its SHA-256 before anything else starts; `artifacts-out`, a native
//! sidecar, waits for the end of the agent (its `SIGTERM`) and sends each output, also
//! when the agent failed. Neither follows a link: a symbolic link left by the agent is
//! skipped, and only regular files go into a folder's archive.

use pnex_core::agent_runs::{ArtifactForm, ARTIFACTS_IN_DIR, ARTIFACTS_OUT_DIR};
use pnex_core::runners::SandboxArtifacts;
use serde_json::{json, Value};

/// Room to send the outputs once the agent is done: the grace period of the pod.
pub const COLLECT_GRACE_SECS: u32 = 300;

/// Fetches `name,hex,form` entries of `$PNEX_ARTIFACTS_LIST` into `$ROOT/in`.
pub const IN_SCRIPT: &str = r#"set -eu
root="${PNEX_ARTIFACTS_ROOT:-/artifacts}"
mkdir -p "$root/in"
for entry in $PNEX_ARTIFACTS_LIST; do
  name="${entry%%,*}"; rest="${entry#*,}"; sha="${rest%%,*}"; form="${rest#*,}"
  tmp="$root/.in-$name"
  curl -fsS --retry 3 -H "Authorization: Bearer $PNEX_ARTIFACT_TOKEN" \
    -o "$tmp" "$PNEX_ARTIFACTS_URL/inputs/$name"
  got="$(sha256sum "$tmp" | cut -d' ' -f1)"
  if [ "$got" != "$sha" ]; then
    echo "input $name: checksum mismatch" >&2
    exit 1
  fi
  if [ "$form" = tar ]; then
    mkdir -p "$root/in/$name"
    # As a non-root user: owners in the archive are not applied.
    tar -xf "$tmp" -C "$root/in/$name"
    rm -f "$tmp"
  else
    mv "$tmp" "$root/in/$name"
  fi
  echo "input $name: ready"
done
# Caches (D730): keyed by the lock files of the workspace, cloned just before.
mkdir -p "$root/.cache-keys"
for entry in ${PNEX_CACHES_LIST:-}; do
  name="${entry%%,*}"; files="${entry#*,}"
  key=none
  if [ -n "$files" ] && [ "$files" != "$entry" ]; then
    key="$(for f in $(echo "$files" | tr ':' ' '); do cat "${PNEX_WORKSPACE:-/workspace}/$f" 2>/dev/null; done | sha256sum | cut -c1-32)"
  fi
  echo "$key" > "$root/.cache-keys/$name"
  dest="${PNEX_CACHES_ROOT:-/caches}/$name"
  mkdir -p "$dest"
  tmp="$root/.cache-$name.tar"
  if curl -fsS --retry 3 -H "Authorization: Bearer $PNEX_ARTIFACT_TOKEN" \
      -D "$tmp.head" -o "$tmp" "$PNEX_ARTIFACTS_URL/caches/$name?key=$key" 2>/dev/null; then
    if tar -xf "$tmp" -C "$dest"; then
      if grep -qi '^x-pnex-cache-exact: *true' "$tmp.head"; then
        touch "$root/.cache-keys/$name.exact"
        echo "cache $name: restored, same key"
      else
        echo "cache $name: restored from an older key"
      fi
    else
      echo "cache $name: unreadable, starting empty" >&2
    fi
  else
    echo "cache $name: none yet"
  fi
  rm -f "$tmp" "$tmp.head"
done
"#;

/// Sends each name of `$PNEX_ARTIFACTS_LIST` found under `$ROOT/out`, at `SIGTERM`.
pub const OUT_SCRIPT: &str = r#"set -u
root="${PNEX_ARTIFACTS_ROOT:-/artifacts}"
send() {
  curl -fsS --retry 3 -X PUT -H "Authorization: Bearer $PNEX_ARTIFACT_TOKEN" \
    -H "x-pnex-artifact-form: $1" -T - "$PNEX_ARTIFACTS_URL/outputs/$2" >/dev/null
}
collect() {
  for name in $PNEX_ARTIFACTS_LIST; do
    p="$root/out/$name"
    if [ -L "$p" ]; then
      echo "output $name: a link, skipped" >&2
    elif [ -f "$p" ]; then
      send file "$name" < "$p" && echo "output $name: sent" || echo "output $name: not sent" >&2
    elif [ -d "$p" ]; then
      (cd "$p" && find . -type f | tar -cf - -T -) | send tar "$name" \
        && echo "output $name: sent" || echo "output $name: not sent" >&2
    fi
  done
}
save_caches() {
  for entry in ${PNEX_CACHES_LIST:-}; do
    name="${entry%%,*}"
    if [ -e "$root/.cache-keys/$name.exact" ]; then
      echo "cache $name: unchanged key, not sent"
      continue
    fi
    key="$(cat "$root/.cache-keys/$name" 2>/dev/null || echo none)"
    src="${PNEX_CACHES_ROOT:-/caches}/$name"
    [ -d "$src" ] || continue
    (cd "$src" && find . -type f | tar -cf - -T -) | curl -fsS --retry 3 -X PUT \
      -H "Authorization: Bearer $PNEX_ARTIFACT_TOKEN" -T - \
      "$PNEX_ARTIFACTS_URL/caches/$name?key=$key" >/dev/null \
      && echo "cache $name: saved" || echo "cache $name: not saved" >&2
  done
}
trap 'collect; save_caches; exit 0' TERM
echo "collector: waiting for the agent"
while :; do sleep 1 & wait $!; done
"#;

fn list_of(entries: impl Iterator<Item = String>) -> String {
    entries.collect::<Vec<_>>().join(" ")
}

/// `(init container, sidecar)` moving the artifacts of a run, each absent when nothing
/// is declared on its side. The token comes from `secret`, key `artifact-token`.
pub fn transfer_containers(
    artifacts: &SandboxArtifacts,
    image: &str,
    secret: &str,
    security: &Value,
    resources: &Value,
) -> (Option<Value>, Option<Value>) {
    let caches = list_of(
        artifacts
            .caches
            .iter()
            .map(|c| format!("{},{}", c.name, c.key_files.join(":"))),
    );
    let env = |list: String| {
        json!([
            { "name": "PNEX_ARTIFACTS_URL", "value": artifacts.url },
            { "name": "PNEX_ARTIFACTS_LIST", "value": list },
            { "name": "PNEX_CACHES_LIST", "value": caches },
            { "name": "PNEX_ARTIFACT_TOKEN",
              "valueFrom": { "secretKeyRef": { "name": secret, "key": "artifact-token" } } },
        ])
    };
    let cache_mounts = |read_only: bool| -> Vec<Value> {
        artifacts
            .caches
            .iter()
            .enumerate()
            .map(|(i, c)| {
                json!({ "name": format!("cache-{i}"), "mountPath": format!("/caches/{}", c.name),
                        "readOnly": read_only })
            })
            .collect()
    };
    let mut fetch_mounts = vec![
        json!({ "name": "artifacts", "mountPath": "/artifacts" }),
        // Read only: the keys of the caches are the lock files cloned there.
        json!({ "name": "workspace", "mountPath": "/workspace", "readOnly": true }),
    ];
    fetch_mounts.extend(cache_mounts(false));
    // The whole volume, read only: its outputs, and the keys the fetch wrote.
    let mut collect_mounts =
        vec![json!({ "name": "artifacts", "mountPath": "/artifacts", "readOnly": true })];
    collect_mounts.extend(cache_mounts(true));
    let fetch = (!artifacts.inputs.is_empty() || !artifacts.caches.is_empty()).then(|| {
        let list = list_of(artifacts.inputs.iter().map(|a| {
            let hex = a.sha256.as_deref().unwrap_or_default();
            let form = a.form.unwrap_or(ArtifactForm::File).as_str();
            format!("{},{},{form}", a.name, hex.trim_start_matches("sha256:"))
        }));
        json!({
            "name": "artifacts-in",
            "image": image,
            "command": ["sh", "-c", IN_SCRIPT],
            "env": env(list),
            "securityContext": security,
            "resources": resources,
            "volumeMounts": fetch_mounts,
        })
    });
    let collect = (!artifacts.outputs.is_empty() || !artifacts.caches.is_empty()).then(|| {
        json!({
            "name": "artifacts-out",
            "image": image,
            "restartPolicy": "Always",
            "command": ["sh", "-c", OUT_SCRIPT],
            "env": env(list_of(artifacts.outputs.iter().map(|a| a.name.clone()))),
            "securityContext": security,
            "resources": resources,
            "volumeMounts": collect_mounts,
        })
    });
    (fetch, collect)
}

/// Where the agent and `setup` find each cache: its own folder, writable.
pub fn cache_mounts_of(artifacts: &SandboxArtifacts) -> Vec<Value> {
    artifacts
        .caches
        .iter()
        .enumerate()
        .map(|(i, c)| json!({ "name": format!("cache-{i}"), "mountPath": c.path }))
        .collect()
}

/// One volume per cache, gone with the pod: S3 alone keeps it between runs.
pub fn cache_volumes_of(artifacts: &SandboxArtifacts) -> Vec<Value> {
    (0..artifacts.caches.len())
        .map(|i| json!({ "name": format!("cache-{i}"), "emptyDir": { "sizeLimit": "8Gi" } }))
        .collect()
}

/// What the agent container mounts: its inputs read only, its outputs writable.
pub fn agent_mounts() -> [Value; 2] {
    [
        json!({ "name": "artifacts", "mountPath": ARTIFACTS_IN_DIR, "subPath": "in", "readOnly": true }),
        json!({ "name": "artifacts", "mountPath": ARTIFACTS_OUT_DIR, "subPath": "out" }),
    ]
}
