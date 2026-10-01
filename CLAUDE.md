# pnex-rust

## Langues

- **Code et commentaires de code : ANGLAIS. RÈGLE ABSOLUE, ZÉRO EXCEPTION. RÉPÉTÉE ET RENFORCÉE (2026-09-22, après récidives).**
  - Toute nouvelle fonction, toute nouvelle ligne de commentaire, tout message de log/erreur ajouté au code (Rust, C++, JS…) est écrit en anglais — MÊME dans un fichier existant dont les commentaires voisins sont en français.
  - **Ne jamais imiter la langue du code environnant : l'ajout reste anglais.** Un doc-comment `///`/`//!` nouveau, une doc de champ de struct, un commentaire de section, un commentaire inline ajouté → anglais, sans exception.
  - Refuser consciemment le réflexe « harmonie avec le fichier » : écrire d'abord le commentaire en anglais, même si la ligne du dessus est en français.
  - Ne pas repasser sur l'existant rien que pour la langue (mais si on réécrit une ligne commentée, le nouveau texte est en anglais).
- Les documents d'architecture, roadmaps et registres de décisions restent en français — on ne les traduit pas

## i18n de l'UI (fr-FR / en-US)

- **Tout texte visible par l'utilisateur passe par `t!`** (locales `crates/pnex-frontend/locales/{fr-FR,en-US}.ftl`, parité obligatoire). Nouvelle clé = **les deux fichiers tout de suite** ; `t!` panique sur clé absente. Jamais de `t!` dans une task détachée hors scope de rendu.
- **Erreurs serveur = code machine + description anglaise canonique.** Nouvelle erreur backend = 1 code dans `pnex_core::err_codes::ALL` + la clé `err-<code-kebab>` dans les deux `.ftl` (garde : `crates/pnex-backend/tests/error_codes.rs`). L'UI résout au render time via `crates/pnex-frontend/src/api/error_i18n.rs` ; code inconnu → repli verbatim (jamais de panic). Erreurs de champ : jetons machine (`required`, `max_length:255`). Violations de flow : `code` + `args`, jamais de message pré-rendu.
- **Exceptions verbatim documentées** : diagnostics runtime (`last_error`, feed debug, erreurs nodes/starlark/device), corps des notifications ws/OTA (anglais canonique), texte libre en base (notes média), `pages/showcase.rs`, détail pont JS flash, filelog take360.
- **Gardes bloquants** : `i18n_guard` (scan anti-chaînes FR en dur côté front), `error_detail_codes_are_registered` (codes serveur), parité + sweep `t!` existants. Une chaîne FR en dur dans l'UI = test rouge.

## graphify

This project has a graphify knowledge graph at `graphify-out/` (~13,6k nodes · ~24k edges — ordre de grandeur, le graphe grossit à chaque `/graphify --update` · EXTRACTED/INFERRED audit trail). Use it by default for understanding the codebase:

- Before answering architecture or codebase questions, read `graphify-out/GRAPH_REPORT.md` for god nodes, community structure, and the D1–D16 decision register
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's edges instead of scanning files
- Raw grep is still fine for exact-string lookups (rename, TODO sweep); the graph wins for relationships, flows, and rationale
- After modifying **code** files, run `graphify update .` to refresh the graph (AST-only, no LLM cost). If **docs/fixtures** changed too, run `/graphify --update` instead (semantic re-extraction)
- If the graph is missing or stale and the question is structural, rebuild with `/graphify`
