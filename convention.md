# Conventions du projet pnex-rust

> Référence unique pour ne pas se perdre. Toute nouvelle décision de nommage
> ou de structure se consigne ici (et dans PROGRESS.md pour l'historique).

## Positionnement

- **La version Rust est la version officielle.** Django était un POC multi-années,
  jamais passé en prod → **pas de parité cosmétique** avec Django (slashs terminaux,
  formes d'URL héritées, etc.). L'API Rust fait référence.
- Ce qui DOIT être conservé de Django : les **contrats fonctionnels** capturés en
  Phase 0 (`docs/contracts/`) — payloads, sémantique, comportements.

## Noms

| Élément | Convention | Exemple |
|---|---|---|
| Service (nom affiché, health, logs) | `pnex-server` | `pnex_core::SERVICE_NAME` |
| Binaire backend | `pnex-server` | `cargo run --bin pnex-server` |
| Crates | préfixe `pnex-` | `pnex-core`, `pnex-backend`, `pnex-frontend`, `pnex-firmware-builder` |
| Répertoires de crates | **identiques aux noms de packages** | `crates/pnex-core/`, `crates/pnex-backend/`… |
| App Loco (`app_name`) | `pnex-server` | `crates/pnex-backend/src/app.rs` |

⚠️ Le nom `og-device-hub` (héritage Django) est **mort** — ne jamais le réintroduire.

## Chemins

- Front buildé par dx → `target/dx/pnex-frontend/<mode>/web/public`, copié par le
  Taskfile vers `crates/pnex-frontend/dist/` (gitignoré) — c'est ce dossier que
  Loco sert (middleware static + fallback SPA).
- Configs Loco : `crates/pnex-backend/config/{development,test,production}.yaml`.
- Chemins relatifs inter-crates : toujours via le nom complet
  (ex. `../pnex-frontend/dist`), jamais `../frontend`.

## Git & CI

- Une branche par phase : `phase-<N>-<slug>` (ex. `phase-1-squelette`).
- **Jamais de commit rouge** : `task check` (natif + wasm32), `task test` et
  `task lint` (clippy `-D warnings`) doivent passer avant tout commit.
- Push au fil de l'eau vers `origin` (github.com:Pnex/pnex-rs.git, opensource).
- Messages de commit en français, impératif, avec contexte de phase.

## Doc & langue

- Docs, commentaires et PROGRESS.md en **français**.
- `docs/architecture/features.md` : règles de features Dioxus (CSR web pur,
  jamais `fullstack`/`ssr`/`server`).
- `pnex-core` doit compiler sur natif **et** wasm32 — zéro dépendance native.

## API

- URLs **sans slash terminal** (convention Rust/Axum, assumée).
- Endpoints préfixés par domaine (`/health/*`, puis `/api/...`, `/ws/...`).
- Ne jamais inventer un endpoint ou un champ : en cas d'ambiguïté sur un contrat
  Django, vérifier `docs/contracts/` ou demander.
