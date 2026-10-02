#!/usr/bin/env python3
"""Post-process graphify-out/graph.json after `graphify update`.

`graphify update` merges into the existing graph and resolves unqualified
calls by name, which leaves three kinds of noise:

1. ghost nodes: their source file was deleted or renamed (file splits,
   migration squash) — `update` never purges them;
2. nodes from paths now listed in .graphifyignore;
3. INFERRED `calls` edges guessed from a bare name: every `.ok()` in the
   workspace attached to one arbitrary `.ok()` definition, `Err(...)` to a
   local `fn err`, or calls between Rust crates that cannot see each other
   (backend -> frontend).

Rules for 3 (INFERRED `calls` only, EXTRACTED edges are never touched):
- drop when an endpoint is a method resolved by name (label `.name()`);
- drop when both endpoints are Rust files of different crates and neither
  crate directly depends on the other (Cargo metadata).

`update` never deletes a symbol either (a renamed function keeps its old
node while its file exists), so `task graph:update` rebuilds the code
layer from scratch and passes the previous graph here: its semantic layer
(docs, decisions, rationale — LLM extracted, not reproducible by `update`)
is carried over onto the fresh code graph.

Usage: python3 scripts/graphify_clean.py [PREVIOUS_GRAPH]
       (then `graphify cluster-only .`)
"""

import fnmatch
import json
import os
import subprocess
import sys

GRAPH = "graphify-out/graph.json"
IGNORE = ".graphifyignore"


def ignore_patterns():
    if not os.path.exists(IGNORE):
        return []
    out = []
    for line in open(IGNORE, encoding="utf-8"):
        line = line.strip()
        if line and not line.startswith("#"):
            out.append(line)
    return out


def ignored(path, patterns):
    for pat in patterns:
        if pat.startswith("**/"):
            if pat[3:].rstrip("/") + "/" in path + "/":
                return True
        elif pat.endswith("/"):
            if path.startswith(pat):
                return True
        elif fnmatch.fnmatch(path, pat):
            return True
    return False


def local_crates():
    """(crate dir -> name, name -> set of direct local deps)."""
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--offline"],
            stderr=subprocess.DEVNULL,
        )
    )
    root = meta["workspace_root"].rstrip("/") + "/"
    dirs, deps = {}, {}
    local = [p for p in meta["packages"] if p["source"] is None]
    names = {p["name"] for p in local}
    for p in local:
        d = os.path.dirname(p["manifest_path"])[len(root):] + "/"
        dirs[d] = p["name"]
        deps[p["name"]] = {x["name"] for x in p["dependencies"] if x["name"] in names}
    return dirs, deps


def crate_of(path, dirs):
    if not path or not path.endswith(".rs"):
        return None
    best = None
    for d in dirs:
        if path.startswith(d) and (best is None or len(d) > len(best)):
            best = d
    return dirs[best] if best else None


def carry_semantic(g, prev_path):
    """Add the previous graph's semantic nodes/edges to the fresh code graph.

    Semantic = a node whose file the AST pass did not extract (docs, yaml…)
    or a non-code node (rationale, document) anchored in a code file.
    """
    prev = json.load(open(prev_path, encoding="utf-8"))
    fresh_ids = {n["id"] for n in g["nodes"]}
    ast_files = {n.get("source_file") for n in g["nodes"] if n.get("file_type") == "code"}
    semantic = {
        n["id"]: n
        for n in prev["nodes"]
        if n["id"] not in fresh_ids
        and (n.get("source_file") not in ast_files or n.get("file_type") != "code")
    }
    g["nodes"].extend(semantic.values())
    known = fresh_ids | set(semantic)
    seen = {(e["source"], e["target"], e["relation"]) for e in g["links"]}
    for e in prev["links"]:
        key = (e["source"], e["target"], e["relation"])
        if (
            (e["source"] in semantic or e["target"] in semantic)
            and e["source"] in known
            and e["target"] in known
            and key not in seen
        ):
            g["links"].append(e)
            seen.add(key)
    have = {h.get("id") for h in g.get("hyperedges", [])}
    g.setdefault("hyperedges", []).extend(
        h for h in prev.get("hyperedges", []) if h.get("id") not in have
    )
    print(f"carried {len(semantic)} semantic nodes from {prev_path}", file=sys.stderr)


def main():
    g = json.load(open(GRAPH, encoding="utf-8"))
    if len(sys.argv) > 1:
        carry_semantic(g, sys.argv[1])
    patterns = ignore_patterns()
    dirs, deps = local_crates()

    def keep_node(n):
        s = n.get("source_file")
        if not s:
            return True
        return not ignored(s, patterns) and os.path.exists(s)

    n0, e0 = len(g["nodes"]), len(g["links"])
    g["nodes"] = [n for n in g["nodes"] if keep_node(n)]
    nodes = {n["id"]: n for n in g["nodes"]}
    g["links"] = [e for e in g["links"] if e["source"] in nodes and e["target"] in nodes]
    e1 = len(g["links"])

    def plausible(e):
        if e.get("confidence") != "INFERRED" or e.get("relation") != "calls":
            return True
        a, b = nodes[e["source"]], nodes[e["target"]]
        if a["label"].startswith(".") or b["label"].startswith("."):
            return False
        ca = crate_of(a.get("source_file"), dirs)
        cb = crate_of(b.get("source_file"), dirs)
        if ca and cb and ca != cb:
            return cb in deps.get(ca, ()) or ca in deps.get(cb, ())
        return True

    g["links"] = [e for e in g["links"] if plausible(e)]
    hyper = []
    for h in g.get("hyperedges", []):
        members = [x for x in h.get("nodes", []) if x in nodes]
        if len(members) >= 2:
            h["nodes"] = members
            hyper.append(h)
    g["hyperedges"] = hyper

    json.dump(g, open(GRAPH, "w", encoding="utf-8"))
    print(
        f"nodes {n0} -> {len(g['nodes'])} | edges {e0} -> {e1} (dangling)"
        f" -> {len(g['links'])} (implausible inferred calls)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
