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

## Sécurité (D130, règle permanente)

- **Toute modification de code passe la revue de sécurité** de `docs/architecture/security.md` §6 avant commit (grille en 9 points), et `/security-review` avant merge d'une branche. Les règles invariantes R1–R20 (§3) sont opposables : un écart = refus, ou justification consignée au registre §7.
- **Modèle de menace à garder en tête** : tout inscrit est owner de son org perso → « member » n'est pas une barrière ; la frontière est l'**org**, puis la **plateforme**. Le runtime de flows exécute du code utilisateur multi-org : il est non fiable, le backend re-vérifie tout ce qu'il reçoit.
- Réflexes : org depuis le principal jamais la requête (R1) ; garde de rôle en tête de chaque handler d'écriture + test viewer → 403 (R2) ; pas d'identifiant/secret dans un DTO de lecture (R4, R16) ; rien d'utilisateur en HTML sur l'origine de l'app (R11–R13) ; registre de nœuds en liste blanche, pas d'env visible du code utilisateur (R5, R6).
- **Dépendances : `task security:deps`** (`cargo deny`, job CI `deny` bloquant) ; toute exception dans `deny.toml` porte sa justification.
- Finding découvert en route = ligne au registre §7 (SEC-n), même s'il n'est pas corrigé dans la même PR.

## Assistant IA : toute fonctionnalité lui est livrée (D142–D145, règle permanente)

Ajouter un nœud de flow ou une fonctionnalité utilisateur **sans mettre l'assistant à jour dans le même commit = travail non fini**, au même titre qu'une clé fluent manquante. Référence : `docs/architecture/ai-assistant.md` §9.

- **Nouveau type de nœud** → une entrée `NodeDoc` dans `crates/pnex-core/src/flow/node_docs.rs` (résumé, champs de config, ports, pièges utilisateur), en anglais. Garde bloquante `every_kind_is_documented`. Jamais de liste manuelle de nœuds dans `services/ai/`. Règle de graphe qui change (payload, métrique, câblage) → `FLOW_AUTHORING_RULES` ; l'exemple `FLOW_EXAMPLE` reste valide (test).
- **Nouvelle page / fonctionnalité / code d'erreur visible** → fiche de connaissance `crates/pnex-backend/assistant-kb/*.md` (dès que la couche 2 de D142 existe ; gardes : routes, nœuds et `err_codes` cités existants, chaque route de premier niveau couverte). Fiches `troubleshooting` = symptôme → cause → geste **dans l'UI**, jamais de CLI/API.
- **Nouvel outil de l'assistant** → règle d'extension §9.3, opposable : écriture via le **service partagé avec le contrôleur HTTP de l'UI** (mêmes validations, mêmes 409) ; `can_write` re-vérifié + org depuis le principal (R1, R2) ; une fiche qui décrit l'outil ; trace UI + deep-link ; test « registre == ensemble autorisé » mis à jour.
- **Consignes actées, jamais assouplies par un ajout** :
  - aucune action physique : ni commande device/OTA/flash, ni écriture de contrôle ou de mémoire Valkey — seul un flow déployé par l'humain agit (D123, A4) ;
  - jamais de deploy, stop ni suppression ; un flow **déployé** n'est pas modifiable par l'assistant (`ai-flow-running`, vérifié côté serveur), un widget de dashboard couplé à un flow déployé non plus (D144) ;
  - concurrence optimiste : l'assistant écrit sur la version qu'il a lue (`expected_version`), jamais sur « la dernière » ;
  - refus outil = code machine + `args` (clé `err-<code>` dans les deux `.ftl`), jamais de message pré-rendu ;
  - aucun secret ni identifiant dans une sortie d'outil (R4, R16) ; conversations privées utilisateur × org (D145).

## Commits : Conventional Commits (règle permanente, 2026-10-07)

- **Format obligatoire** : `type(scope): description` — type en **minuscules**, scope optionnel en minuscules sans espace (plusieurs : `ui/mobile`), description en anglais, impérative ou constat court, sans point final, ≤ 72 caractères. Corps libre après une ligne vide (le pourquoi, pas le quoi).
- **Types** : `feat` (fonctionnalité visible), `fix` (bug), `perf`, `refactor` (sans changement de comportement), `docs`, `test`, `build` (deps, Dockerfile, Taskfile), `ci` (workflows), `chore` (le reste), `style`, `revert`. Correctif de sécurité = `fix(security): …` (section « Security » des notes).
- **Rupture** : `feat(api)!: …` + pied `BREAKING CHANGE: <ce qui casse et la migration>`.
- Exemples : `fix(ui): dialog submit enabled while typing`, `feat(flows): device write sends custom commands (D146)`, `docs(firmware): lost command investigation`.
- Interdits : l'ancien style `Fix (scope): …` (majuscule + espace), un sujet sans type, `wip`, `misc`, `update`.
- **Les notes de release en dépendent** : `cliff.toml` (git-cliff) groupe les commits par type entre deux tags `v*` ; le job `release` de `apps.yml` les publie comme corps de la release GitHub. Aperçu local : `task changelog` (non publiés) / `task changelog -- --latest` (dernier tag). Un commit mal formé finit dans « Other ».

## Builds lourds : jamais nus (règle permanente, 2026-10-09)

Cause réelle, 5ᵉ fois : un build Rust lancé depuis un terminal vit dans le cgroup du terminal ; quand il sature la mémoire, `systemd-oomd` tue **tout le scope Warp** — tous les onglets, toutes les sessions (`journalctl --user | grep oomd`). Le même jour, le disque plein a fait planter le linker et affamé Postgres.

- **Tout `cargo build|check|test|clippy|run`, `dx build|serve`, `docker buildx`, `pio run` passe par `scripts/guarded.sh <commande>`** (ou par une tâche `task` qui l'utilise déjà). Le garde : scope systemd dédié plafonné (`MemoryMax` 24G, sans swap) → seul le build est tué, jamais le terminal ; `nice`/`ionice` + score OOM élevé ; refus sous 25 Go de disque libre ; **un seul build lourd à la fois sur la machine** (verrou partagé entre sessions et worktrees, attente automatique).
- Processus longs (serveur de dev, `dx serve`) : `PNEX_GUARD_NO_LOCK=1 scripts/guarded.sh …` après avoir compilé sous verrou.
- `.cargo/config.toml` impose `jobs = 6` et `debug = "line-tables-only"` (binaires de test ÷ 3, link moins gourmand). Ne pas monter `CARGO_BUILD_JOBS` au-delà de 6.
- **Agents/sous-agents : jamais deux compilations en parallèle** (pas de sous-agent qui compile pendant qu'on compile) ; vérifier `df -h` avant une passe complète, nettoyer avec `task clean:incremental` puis `task clean:size`.

## Formatage (hygiène, bloquant en CI)

- **`task fmt` avant chaque commit, `task fmt:check` = job CI `fmt`.** Rust → `cargo fmt` ; intérieur des `rsx!` → `dx fmt` via le wrapper gardé `crates/pnex-frontend/scripts/rsx_fmt.py`.
- **Jamais `dx fmt` à la main** (bugs de recollage : commentaires déplacés/dupliqués, tokens perdus, non-convergence ; `--check` réécrit les fichiers). Détail : `docs/architecture/rsx-fmt.md`.
- Fichier refusé par `task fmt` → réécrire la construction fautive (liste dans rsx-fmt.md : `{match … => rsx!{}}` en bloc, expression multi-ligne en attribut, handler `let … else { return; };` sur une ligne, `with_mut` multi-ligne…). Ne jamais contourner la garde.

## graphify

This project has a graphify knowledge graph at `graphify-out/` (~12,7k nodes · ~22k edges au 2026-10-02 · EXTRACTED/INFERRED audit trail ; `vendor/CoolProp` et `vendor/patches` exclus via `.graphifyignore`, `vendor/edgelinkd` inclus). Use it by default for understanding the codebase:

- Before answering architecture or codebase questions, read `graphify-out/GRAPH_REPORT.md` for god nodes and community structure (decision register: `docs/inventory.md` §0 for D1–D72, then the domain docs in `docs/architecture/*.md` for D73–D121)
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's edges instead of scanning files
- Raw grep is still fine for exact-string lookups (rename, TODO sweep); the graph wins for relationships, flows, and rationale
- After modifying **code** files, run `task graph:update` (AST-only, no LLM cost): rebuilds the code layer from scratch, keeps the semantic layer, drops name-guessed calls (`scripts/graphify_clean.py`). Never bare `graphify update .`: it merges and never deletes (ghost nodes of deleted files and renamed symbols). If **docs/fixtures** changed too, run `/graphify --update` then `task graph:update` (semantic re-extraction, then cleanup)
- If the graph is missing or stale and the question is structural, rebuild with `/graphify`
