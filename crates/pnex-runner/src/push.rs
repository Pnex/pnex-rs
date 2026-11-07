//! Output of a coder agent (D388, D707): its changes to a cloned repository go out as a
//! branch of the run, pushed by a native sidecar of its own. Only that sidecar holds the
//! write token, the agent never does. The sidecar clones the base ref into a volume the
//! agent cannot reach before the agent starts, then, on the end of the agent (its
//! `SIGTERM`), commits the agent's tree as it stands, read only, and pushes the branch.
//!
//! Git never runs inside the repository the agent touched: the agent's `.git` (hooks,
//! `credential.helper`, `url.*.insteadOf`) could send the token elsewhere. The agent's
//! folder is only the work tree of our own git directory: git reads its files, stores
//! symbolic links as links without following them, and skips any `.git` in it.

use pnex_core::runners::SandboxRepo;
use serde_json::{json, Value};

/// Branches a run may push: never the base or a default branch (D388).
pub const BRANCH_PREFIX: &str = "pnex/run-";

/// `$1` base ref, `$2` url, `$3` agent's folder, `$4` branch. Token in `$GIT_TOKEN`.
pub const PUSH_SCRIPT: &str = r#"set -u
base="$1"; url="$2"; tree="$3"; branch="$4"
case "$branch" in pnex/run-*) ;; *) echo "push: branch $branch refused" >&2; exit 1;; esac
[ "$branch" = "$base" ] && { echo "push: the base branch is never pushed" >&2; exit 1; }
root="${PNEX_PUSH_ROOT:-/push}"
export HOME="$root/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
mkdir -p "$HOME"
git_() {
  git -c 'credential.helper=!f() { echo username=x-access-token; echo "password=$GIT_TOKEN"; }; f' \
    -c core.hooksPath=/dev/null -c core.fsmonitor=false -c protocol.allow=never \
    -c protocol.https.allow=always "$@"
}
if ! git_ clone --quiet --depth 1 --no-recurse-submodules --branch "$base" -- "$url" "$root/repo"; then
  echo "push: base not cloned, nothing will be pushed" >&2
  [ -n "${PNEX_PUSH_NOW:-}" ] && exit 0
  trap 'exit 0' TERM
  while :; do sleep 1 & wait $!; done
fi
push() {
  g() { git_ --git-dir="$root/repo/.git" --work-tree="$tree" "$@"; }
  if [ ! -d "$tree" ]; then echo "push: no folder $tree" >&2; exit 0; fi
  g add -A . || { echo "push: changes not read" >&2; exit 0; }
  if g diff --cached --quiet; then echo "push: no change"; exit 0; fi
  g -c user.name="PneX agent" -c user.email=agent@pnex.invalid \
    commit --quiet --no-verify -m "Changes of agent run ${branch#pnex/run-}" \
    || { echo "push: not committed" >&2; exit 0; }
  if g push --quiet --no-verify origin "HEAD:refs/heads/$branch"; then
    echo "push: branch $branch sent"
  else
    echo "push: branch $branch not sent" >&2
  fi
  exit 0
}
# A micro-VM is over when the runner pushes from the host (D762): no agent to wait for.
[ -n "${PNEX_PUSH_NOW:-}" ] && push
trap push TERM
echo "push: waiting for the agent"
while :; do sleep 1 & wait $!; done
"#;

/// The push sidecar of repository `i`, when the run asked for a branch. Its write token
/// goes into the run's Secret under `push-token-<i>`, read by this container alone.
pub fn push_container(
    i: usize,
    repo: &SandboxRepo,
    image: &str,
    secret: &str,
    security: &Value,
    resources: &Value,
) -> Option<(Value, Value, (String, String))> {
    let push = repo.push.as_ref()?;
    let key = format!("push-token-{i}");
    let volume = format!("push-{i}");
    let container = json!({
        "name": format!("push-{i}"),
        "image": image,
        "restartPolicy": "Always",
        "command": [
            "sh", "-c", PUSH_SCRIPT, "push",
            repo.git_ref, repo.url, format!("/workspace/{}", repo.path), push.branch
        ],
        "env": [{
            "name": "GIT_TOKEN",
            "valueFrom": { "secretKeyRef": { "name": secret, "key": key } }
        }],
        "securityContext": security,
        "resources": resources,
        "volumeMounts": [
            { "name": "workspace", "mountPath": "/workspace", "readOnly": true },
            { "name": volume, "mountPath": "/push" },
        ],
    });
    let own = json!({ "name": volume, "emptyDir": { "sizeLimit": "4Gi" } });
    Some((container, own, (key, push.token.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn sh(script: &str, args: &[&str]) -> std::process::Output {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("push")
            .args(args)
            .output()
            .unwrap()
    }

    #[test]
    fn only_run_branches_other_than_the_base_are_pushed() {
        for (base, branch) in [
            ("main", "main"),
            ("main", "feature"),
            ("pnex/run-1", "pnex/run-1"),
        ] {
            let out = sh(
                PUSH_SCRIPT,
                &[
                    base,
                    "https://example.invalid/r.git",
                    "/nonexistent",
                    branch,
                ],
            );
            assert_eq!(out.status.code(), Some(1), "{base} {branch}");
        }
    }

    #[test]
    fn the_write_token_stays_in_the_secret_and_the_agent_tree_is_read_only() {
        let repo = SandboxRepo {
            url: "https://forge.example/o/r.git".into(),
            git_ref: "main".into(),
            path: "r".into(),
            token: None,
            push: Some(pnex_core::runners::SandboxPush {
                branch: "pnex/run-abc".into(),
                token: "ghp_write".into(),
            }),
        };
        let (c, own, (key, token)) = push_container(
            0,
            &repo,
            "init@sha256:x",
            "job-secret",
            &json!({}),
            &json!({}),
        )
        .unwrap();
        assert!(!c.to_string().contains("ghp_write"));
        assert_eq!(
            (key.as_str(), token.as_str()),
            ("push-token-0", "ghp_write")
        );
        assert_eq!(
            c["env"][0]["valueFrom"]["secretKeyRef"]["key"],
            "push-token-0"
        );
        assert_eq!(c["volumeMounts"][0]["readOnly"], true);
        assert_eq!(own["name"], "push-0");
        let no_push = SandboxRepo { push: None, ..repo };
        assert!(push_container(0, &no_push, "i", "s", &json!({}), &json!({})).is_none());
    }
}
