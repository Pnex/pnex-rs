# Architecture — découpage des crates et features

> Phase 1. Décisions structurelles du workspace, à respecter dans toutes les
> phases suivantes.

## Le workspace

| Crate | Rôle | Cibles |
|---|---|---|
| `pnex-core` | DTO/constantes partagés backend ↔ frontend. **Aucune dépendance native** (pas de tokio, std::net, std::fs…). Serde uniquement. | natif **et** wasm32 |
| `pnex-backend` | API Loco/Axum, WS ingestion, worker, serving statique du front. Binaire `pnex-server`. | natif |
| `pnex-frontend` | Web UI Dioxus **CSR pur**. Appelle l'API Loco via HTTP (gloo-net) et WS. | wasm32 (web) |
| `pnex-firmware-builder` | Lib d'orchestration des builds firmware (Phase 6). | natif |

## Le piège « fullstack Dioxus » — et pourquoi on ne l'utilise pas

Dioxus propose un mode `fullstack`/`ssr` avec **server functions** (`#[server]`)
et un serving Axum intégré au runtime Dioxus. Notre backend est **Loco**, qui
est aussi Axum. Activer les deux :

- **collision de servings** : deux stacks Axum/tower concurrentes pour une
  seule application — incompatibles à composer proprement ;
- **hydration SSR/CSR** : le rendu serveur exige que le binaire serveur et le
  wasm partagent exactement le même rendu — contrainte forte, fragile, et
  inutile pour un tableau de bord IoT derrière auth ;
- verrouillage sur le runtime Dioxus pour la partie serveur, alors que la
  valeur de Loco (ORM, migrations, workers, config) est côté serveur.

**Décision** : `pnex-frontend` est CSR web pur. Les features Dioxus
`server`/`fullstack` ne sont **jamais** activées. La communication avec le
backend est du HTTP/WS explicite, comme le ferait un client externe — même
contrat que les devices et les tests de parité.

## Cibles desktop/ios/android — architecture préparée, build pas activé

Directive (2026-08-15) : une app desktop/mobile PNEX est **prévue**. Sur ces
cibles le front n'est PAS servi par le backend (pas de same-origin) : l'URL
du serveur auto-hébergé est renseignée par l'utilisateur, « façon Bitwarden ».
L'architecture est déjà en place :

- `api/config.rs` — seam unique de résolution de la base URL : web = URLs
  relatives (same-origin, ou `PNEX_API_BASE_URL` à la compilation pour le
  dev hot) ; natif = env d'exécution puis préférence stockée
  (`pnex.api_base`) ;
- `pages/server_url.rs` — écran « URL du serveur » (non routé tant que la
  cible n'existe pas) ;
- `storage.rs` — implémentation mémoire native (persistance fichier à la
  phase desktop) ;
- le login PKCE est réorientable (webview dédiée / schéma custom — à trancher
  en phase desktop).

⚠️ Point connu pour cette future phase : le client HTTP vit en `thread_local`
car les futurs reqwest sont `!Send` en wasm ; sur desktop le runtime spawn
pourra exiger des futurs `Send` — il faudra rendre la couche `Send` côté
natif (cfg) ou changer d'organisation. De même, activer `dioxus/desktop`
restera une **décision explicite de phase** (le commentaire « jamais »
initial est caduc depuis la directive desktop — le principe « pas de
glissement silencieux » reste).

## Features du frontend

```toml
[features]
default = ["web"]
web = ["dioxus/web"]
router = ["dioxus/router"]
```

- `web` uniquement. Pas de feature `ssr`/`server` — le piège documenté ci-dessus.
- desktop/mobile : cf. section précédente — phase explicite ultérieure.

## Styling et i18n (Phase 3)

- **Tailwind CSS v4** : source `crates/pnex-frontend/style/tailwind.css`
  (`@import "tailwindcss" source(none)` + `@source "../src"`), générée vers
  `assets/tailwind.css` (gitignoré) par `task css:build` — **avant tout build
  dx** (toujours passer par la Taskfile). CI : step npm (`npm ci` +
  `npm run css:build`) dans le job `dx build`.
- **i18n Fluent** (`dioxus-i18n`) : locales `locales/{fr-FR,en-US}.ftl`
  embarquées, zéro libellé en dur, parité des clés testée
  (`i18n::tests::parite_cles_fr_en`).

## Serving du front

- `dx build --platform web` sort dans `target/dx/pnex-frontend/<mode>/web/public` ;
- le Taskfile copie ce dossier vers `crates/pnex-frontend/dist` (gitignoré) ;
- Loco sert `crates/pnex-frontend/dist` via le middleware `static` avec fallback
  `index.html` (SPA) — cf. `crates/pnex-backend/config/*.yaml`.

En boucle de dev front pure : `task dev:hot` (dx serve :5151 + `task css:watch`,
hot-reload). Le backend tourne à part (`task dev:backend` :5150) ; le CORS dev
verrouillé sur l'origine :5151 est dans `development.yaml` **uniquement**.

## Règles de garde (à ne pas briser)

1. `pnex-core` doit toujours compiler sur les **deux** cibles — `task check`
   le vérifie, la CI l'impose.
2. Le backend ne dépend jamais du frontend ; le frontend dépend de `pnex-core`
   uniquement (jamais du backend).
3. Aucune feature Dioxus serveur ne peut apparaître dans `pnex-frontend`.
