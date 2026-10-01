# Images conteneur PNeX (D71)

> Décision utilisateur 2026-09-27, dans la foulée du TLS edge (D70) :
> toute la chaîne PNeX tourne en conteneurs, sur des bases **Chainguard**
> (surface minimale, CVE corrigées en continu), pour le Pi comme pour le
> cloud.

## Deux images, un seul binaire

| Image | Base | Contenu | Commande |
|---|---|---|---|
| `pnex-server` | `cgr.dev/chainguard/glibc-dynamic` — **ni shell ni gestionnaire de paquets**, non-root (65532) | `pnex-server`, `pnex-flow-runtime` (supervisé, trouvé via PATH), front `dist` | `start` (serveur seul) |
| `pnex-builder` | `cgr.dev/chainguard/wolfi-base` (glibc) | même `pnex-server` + PlatformIO 6.2.0 + esptool 5.4.0 + plateformes/toolchains pré-installées (ESP8266, ESP32, C3, S3) + modèles ONNX | `start --worker` |

Pourquoi deux : le worker de build firmware lance `pio` / `esptool`
(Python + toolchains glibc) → impossible en distroless. Le serveur, lui,
ne dépend que de glibc + libstdc++ (vérifié `ldd`) → image sans shell.
Les deux se partagent la file de jobs en base (loco `BackgroundQueue`) :
`build_firmware` et `stitch_panorama` tournent dans `pnex-builder`. C'est
le premier pas de la fabric de workers (worker-fabric.md).

## Choix écartés (mesurés)

- **Alpine** : musl — les toolchains PlatformIO sont des binaires glibc.
- **CentOS** : CentOS Linux en fin de vie (2024) ; Stream = amont de RHEL.
  Aucun gain de surface vs Debian/UBI.
- **Wolfi validé** (2026-09-27) : `pio run` ESP8266 (20 s), ESP32 (31 s),
  ESP32-C3 (14 s) → SUCCESS ; esptool 5.4.0 OK.
- **glibc-dynamic validé** : le binaire debug local s'y exécute tel quel.

## Build

`deploy/docker/Dockerfile`, contexte = racine du repo (`.dockerignore`
exclut `target/`, `.pio`, venvs, `vendor/edgelinkd/target`…) :

1. `assets` (node:22-trixie-slim) : Tailwind + bundles esbuild.
2. `build` (rust:1.96-trixie, **non livré**) : `dx build --release`
   (dioxus-cli 0.7.10 via binstall) puis `cargo build --release` du
   serveur et du runtime. Trixie obligatoire : le `dx` précompilé exige
   glibc ≥ 2.39 (bookworm = 2.36). Binaires liés à glibc 2.41, images
   Chainguard en 2.44 (compatibilité ascendante). `target/` en cache
   BuildKit.
3. `server` / `builder` : copie des artefacts.

`task docker:build [TAG=…]`.

## Exécution

- `compose.app.yaml` (surcouche) ajoute `pnex-server` (+ port 5150) et
  `pnex-builder` à l'infra de `compose.yaml`, volume `/data` partagé
  (`flow-state`, `media`).
- `task app:up` — conteneurs derrière rien ; `task app:up EDGE=1` — edge
  TLS devant (`PNEX_EDGE_BACKEND=container` : nginx vise
  `pnex-server:5150`) ; `task app:down`.
- Config : `LOCO_ENV=production` → `production.yaml` complété (migration
  au boot `PNEX_AUTO_MIGRATE`, `issuer_url` si `RAUTHY_ISSUER_URL`, section
  `flow` active par défaut, debug tools off).
- nginx résout ses upstreams **à chaque requête** (DNS docker) : il
  démarre avant `pnex-server` et suit la nouvelle IP d'un conteneur
  recréé. Backend sur l'hôte : IP littérale de la passerelle du réseau
  compose (calculée par `apply-edge.sh`).

## Limites / suite

- Secrets de `compose.app.yaml` = valeurs DEV par défaut ; à surcharger
  (tokens notify/flow, mot de passe O2).
- **arm64 (Pi)** : non construit à ce stade — buildx multi-arch à ajouter
  (toolchains PlatformIO linux_aarch64 existantes).
- Tags Chainguard gratuits = `latest` seulement → épingler par digest
  pour des builds reproductibles.
- Les `lib_deps` PlatformIO restent téléchargées à chaque build (projet
  copié en répertoire temporaire) — réseau requis côté builder.
