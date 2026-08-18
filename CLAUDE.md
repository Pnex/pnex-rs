# pnex-rust

## graphify

This project has a graphify knowledge graph at `graphify-out/` (2082 nodes · 2943 edges · EXTRACTED/INFERRED audit trail). Use it by default for understanding the codebase:

- Before answering architecture or codebase questions, read `graphify-out/GRAPH_REPORT.md` for god nodes, community structure, and the D1–D16 decision register
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's edges instead of scanning files
- Raw grep is still fine for exact-string lookups (rename, TODO sweep); the graph wins for relationships, flows, and rationale
- After modifying **code** files, run `graphify update .` to refresh the graph (AST-only, no LLM cost). If **docs/fixtures** changed too, run `/graphify --update` instead (semantic re-extraction)
- If the graph is missing or stale and the question is structural, rebuild with `/graphify`
