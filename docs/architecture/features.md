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
`desktop`/`mobile`/`server`/`fullstack` ne sont **jamais** activées. La
communication avec le backend est du HTTP/WS explicite, comme le ferait un
client externe — même contrat que les devices et les tests de parité.

## Features du frontend

```toml
[features]
default = ["web"]
web = ["dioxus/web"]
```

- `web` uniquement. Pas de feature `ssr`/`server` — le piège documenté ci-dessus.
- Si un jour une cible desktop/mobile est voulue, ce sera une décision
  explicite de phase, pas un glissement silencieux.

## Serving du front

- `dx build --platform web` sort dans `target/dx/pnex-frontend/<mode>/web/public` ;
- le Taskfile copie ce dossier vers `crates/frontend/dist` (gitignoré) ;
- Loco sert `crates/frontend/dist` via le middleware `static` avec fallback
  `index.html` (SPA) — cf. `crates/backend/config/*.yaml`.

En boucle de dev front pure : `task dev:hot` (dx serve, hot-reload, port dx).
Le backend tourne à part (`task dev:backend`, port 5150).

## Règles de garde (à ne pas briser)

1. `pnex-core` doit toujours compiler sur les **deux** cibles — `task check`
   le vérifie, la CI l'impose.
2. Le backend ne dépend jamais du frontend ; le frontend dépend de `pnex-core`
   uniquement (jamais du backend).
3. Aucune feature Dioxus serveur ne peut apparaître dans `pnex-frontend`.
