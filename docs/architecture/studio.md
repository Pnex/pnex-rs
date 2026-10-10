# Studio — éditeur et viewer de parcours de visite 3D (2026-09-11)

> Éditeur de parcours multi-étages (style 3D Vista) : plans d'étage
> importés, scènes = panoramas 360 de la bibliothèque média posées sur les
> plans, hotspots de navigation, publication (viewer in-app + lien public
> tokenisé). Mode panorama V1 — la maquette 3D (three.js) est une tranche
> ultérieure.
>
> Fondations : `media.md` (D21 — assets **référencés, jamais dupliqués**),
> `flow-engine.md` (D18 — documents versionnés append-only),
> `inventory.md` (D2 org-tenant, D14 pagination). Contraintes transverses :
> Dioxus **CSR pur**, endpoints **additifs** — ne jamais bumper
> `pnex_api_contract::CONTRACT`, octets **jamais en base** (MediaStore
> fs/RustFS — MinIO banni).

## 0. Décisions (espace Sx — la numérotation D est partagée avec
`viz-bases.md` de la branche non fusionnée `worktree-viz-concept`, on ne
doubliera pas)

| # | Décision | Pourquoi |
|---|----------|----------|
| S1 | **Domaine autonome sur main** : `tours`/`tour_versions` + document JSONB ; PAS de tables `sites`/`buildings`/`floors` (le tour porte ses étages dans son doc) | Le studio ne dépend pas de la branche viz ; étages = structure du parcours, pas de la cartographie générale |
| S2 | **Mode panorama V1** (`tours.mode`, validé `panorama`) — scènes = panoramas 360 (pannellum), posés à la main sur des plans importés ; maquette three.js = tranche suivante | 3D Vista-like : les assets 360 existants (takes stitchés) deviennent des visites ; pas de dépendance WebGL2 lourde |
| S3 | **Versioning école flows** : `tour_versions` append-only, save = PATCH `expected_version_number` → 409 ; **la version courante = la dernière** (pas de colonne `current_version_number`, pas d'endpoint restore — « restaurer » = charger une vieille version dans l'éditeur puis save) | Précédent prouvé (`flows`/`flow_editor`) ; une colonne courante + restore dupliqueraient la sémantique latest sans usage réel |
| S4 | **Publication = pointeur** `tours.published_version_id` → `tour_versions.id` (FK circulaire SQL brut **PG-only** `ON DELETE SET NULL`, intégrité par le contrôleur sur sqlite — école `flows.deployed_version_id`/`media_assets.current_version_id`) | Publier fige une version sans empêcher l'édition ; la re-publication d'une version antérieure est un simple repointage |
| S5 | **Share public tokenisé** : `tours.share_token` varchar(64) UNIQUE (32 hex = `Uuid::new_v4().simple()`, 122 bits) ; `POST /share` exige publié (409 `not_published`) ; régénérer tue l'ancien ; `unpublish` = publication **et** token NULL ; `DELETE /share` révoque le lien en gardant la publication | Lien partageable sans login (façon export 3D Vista) ; un seul interrupteur « public » à dépublier ; 404 uniforme (pas d'énumération) |
| S6 | **Étages/scènes/liens dans `tour_versions.doc`** (`pnex_core::TourDoc`) : `floors[]` (plan optionnel : image XOR rien), `scenes[]` (référence `media_asset_id` kind `panorama`, x/y en px natifs du plan, vue initiale), `links[]` (hotspots yaw/pitch, `kind` `walk\|stair` **dérivé** des étages des extrémités, jamais saisi) | Un save atomique par version ; pas de CRUD étages séparé ; le doc reste la seule source de vérité du parcours |
| S7 | **Assets référencés, jamais dupliqués** (D21) : les références du doc sont des UUID `media_assets.id` **en string** (pnex-core reste sans dep uuid — wasm32-safe) ; références mortes **tolérées au rendu** (hotspot/plan désactivé + toast, jamais panic) | Doctrine D21 ; un asset supprimé ne casse jamais le viewer |
| S8 | **Validation en deux étages** : structurelle pure `pnex_core::validate_tour_doc` (ids uniques, liens connus, fov/pitch bornés, dims > 0) + **en base** `services/tour.rs::validate_doc_assets` (chaque asset existe dans l'org, kind requis : `panorama` scènes / image plate quelconque — `floorplan` ou `photo` — pour les plans) | École `validate_graph` + `validate_flow_write` ; messages français affichables (`TourViolation`) |
| S9 | **Plan d'étage = `media_assets` kind `floorplan`** — extension de la liste `KINDS` **sans migration** (kind = string applicatif) ; upload par le endpoint média existant (octet-stream, pas de nouvel endpoint) ; sniff ne reclassifie pas un floorplan (kind client prioritaire — règle existante) | École D25 de viz-bases.md ; les plans sont versionnés par le système média (exigence initiale du studio) |
| S10 | **Endpoint public sans auth** : `controllers/public_tours.rs` (aucun extracteur OrgContext, école `health.rs`) — `GET /{token}` (doc publié + carte des assets : version **courante** de chacun) et `GET /{token}/assets/{asset_id}?v=n` (octets si l'asset est référencé par le doc publié **et** dans l'org du tour) | Le viewer partagé boote en un aller ; la référence par le doc publié est l'autorisation de lecture |
| S11 | **Cache** : doc public `Cache-Control: no-store` (le même token sert un doc différent après re-publication) ; octets `public, max-age=31536000, immutable` (URL versionnée `?v=n`, versions média immuables append-only) | Correctness vs CDN/navigateur |
| S12 | **Viewer = extension de `viewers.js`** (`window.pnexViewers.tour = {mount, switch, unmount}` enveloppant pannellum déjà présent) — PAS de bundle séparé ; hotspots `type:"info"` dont la closure JS pose `window.__pnexTourLastNav` (chaîne JSON `{seq, scene_id, floor_id}`) lue par polling Rust (`take_nav`) | Un seul pannellum chargé (pas de doublon de bundle) ; pattern global-chaîne-JSON prouvé (map_viewer viz, tron) ; natif Android : glue JS 100 % synchrone (eval n'attend pas les promesses) |
| S13 | **Mémoire : une seule scène montée à la fois** — `switch_scene` = destroy + re-mount pannellum ; fetch des octets côté Rust (`media_blob_url`) par scène à la demande, pas de préchargement V1 | Data-URI natif (+33 % base64) limité à un fichier ; ~1 texture en mémoire |
| S14 | **`share_token` peuplé dans les DTOs uniquement si `can_write()`** ; `published_version_number`/`share_enabled` visibles à tous les membres | Le token est un secret de publication ; l'état, lui, est interne à l'org |
| S15 | **Liens auto + hotspots fléchés** (2026-09-12) : `add_scene` **auto-lie** (paire aller-retour `state::connect_scenes` vers la plus proche du même étage) ; création partout = **paire** (auto-link, inspecteur « Ajouter », mode lien canvas) ; **angles auto** — yaw = azimut ENU de la direction plan, pitch = **±30° si les niveaux diffèrent** (stair : descendre/monter), 0° sinon ; hotspots viewer = **flèches PNeX** (`cssClass` pannellum + `hotspots.css` : bleu = walk, violet = stair ↑/↓ selon le pitch) | Un parcours est connecté par défaut (zéro mode lien obligatoire) ; la flèche « penche » d'elle-même selon l'étage cible ; toujours éditable (suppression unitaire par direction) |

## 1. Modèle de données

Migration `m20260911_000017_tours.rs` (école `m20260909_000012_media`) :

- **`tours`** — `id` UUID PK, `org_id` BIGINT NN FK CASCADE, `name`
  varchar(255) NN, `description` TEXT NULL, `mode` varchar(16) NN default
  `panorama`, `share_token` varchar(64) NULL UNIQUE (les NULL multiples
  sont admis), `published_version_id` UUID NULL (FK circulaire), `metadata` JSONB NULL, timestamps.
- **`tour_versions`** — `id` UUID PK, `tour_id` UUID NN FK CASCADE,
  `version_number` BIGINT NN, `doc` JSONB NN (`pnex_core::TourDoc`),
  `author` varchar(255) NULL, `note` TEXT NULL, timestamps.
- Index : `uniq_tour_versions_tour_number (tour_id, version_number)` —
  `idx_tours_org` — `uniq_tours_share_token`.

### `tour_versions.doc` (mode panorama)

```json
{
  "mode": "panorama",
  "start_scene": "s1",
  "floors": [
    {"id": "f1", "name": "RDC", "level": 0,
     "plan": {"media_asset_id": "<uuid floorplan>", "width": 2048.0,
              "height": 1024.0, "scale_m_per_px": 0.05} | null,
     "north_deg": 0.0}
  ],
  "scenes": [
    {"id": "s1", "floor_id": "f1", "label": "Salon",
     "media_asset_id": "<uuid panorama>",
     "x": 512.0, "y": 300.0,
     "initial_yaw": 0.0, "initial_pitch": 0.0, "initial_fov": 100.0}
  ],
  "links": [
    {"id": "l1", "from": "s1", "to": "s2", "yaw": 90.0, "pitch": 0.0,
     "kind": "walk", "label": "Vers cuisine"}
  ]
}
```

Types partagés backend ↔ front dans `crates/pnex-core/src/tour.rs`
(serde-only, école `flow.rs`) + DTOs (`TourSummary`, `TourDetail`,
`TourVersionSummary`, `CreateTour`, `UpdateTour`, `PublishTour`,
`PublicTour`).

## 2. API

Authentifié (`controllers/tours.rs`, `/api/v1/tours`, OrgContext) :
`GET ""` (D14, search name + filtre mode), `POST ""` (tour + v1 minimal),
`GET /{id}[?version=n]`, `PATCH /{id}` (save append-only, 409 optimiste),
`DELETE /{id}` (cascade versions — **aucun blob à purger**, les assets sont
référencés), `GET /{id}/versions[/ {n}]`, `POST /{id}/publish`
(`version_number` absent = latest), `POST /{id}/unpublish`,
`POST|DELETE /{id}/share`. Contrat : `docs/contracts/tours.http`.

Public (`controllers/public_tours.rs`, `/api/v1/public/tours`, sans auth) :
`GET /{token}` + `GET /{token}/assets/{asset_id}?v=n` — 404 uniforme
(S10/S11).

Kind média **`floorplan`** ajouté à `KINDS` (`controllers/media.rs`) —
aucune migration, aucun nouvel endpoint d'upload (S9).

## 3. Frontend (Dioxus CSR)

- Routes **statiques** : `#[route("/studio")]` dans le layout Shell (nav) ;
  `#[route("/share/:token")]` **hors layout** (précédent `AuthCallback`) —
  viewer public sans login ; fetch public sans Bearer
  (`PUBLIC_PREFIX` dans `api/client.rs`).
- `pages/studio.rs` : liste ↔ `TourEditor` par signal (école
  `pages/flows.rs`, GlobalSignal `OPEN_TOUR`) ; `components/tour_editor/`
  sur le squelette `flow_editor` (`mod.rs` EditorCx + `Interaction`,
  `geometry.rs` pur testé, `state.rs` réducteurs purs, `canvas.rs` SVG sans
  viewBox, `inspector.rs` par `key`, `versions.rs` drawer + publication) ;
  `media_picker.rs` (choix d'asset média par kind).
- Viewer : `src/tour_viewer.rs` (pont wasm/natif, école `media_viewer.rs` :
  retry 100 ms × 50, `Reflect` wasm / `document::eval` natif, échec → badge
  jamais panic) + `components/tour_viewer.rs` (sélecteur d'étages + mini-plan
  + polling `take_nav` ~300 ms) ; une seule scène montée (S12/S13).
- i18n : registre `studio-*`/`tour-*` (parité fr/en testée) ; toasts =
  erreurs serveur relayées verbatim.

## 4. ⚠ Collision de numérotation avec la branche viz

La branche **non fusionnée** `worktree-viz-concept` (PRD
`viz-bases.md`, D22–D34) porte une AUTRE migration `000014` (domaine
`sites`, tables `tours` avec `site_id` NOT NULL → `sites`). À la convergence
: renuméroter une des deux migrations (précédent : ai_connectors 000010) et
réconcilier le schéma `tours`. Décisions du studio numérotées **Sx** pour ne
pas doubler D22–D34. La base de dev partagée `pnex` contient déjà les tables
viz : le studio utilise sa propre base dev `pnex_studio` et sa base de test
`pnex_test_studio`.

## 5. Reste ouvert (tranches suivantes)

1. Maquette 3D (three.js, `tours.mode = maquette`) — école D28 de viz.
2. Préchargement/LRU des blob URLs de panoramas (polish perfs).
3. Éditeur de vue initiale « depuis le panorama » (poser yaw/pitch en
   regardant la scène) — V1 = champs numériques.
4. Export offline du tour (bundle autonome) — non planifié.
5. Convergence viz : fusion des deux domaines (floors DB ↔ étages doc) —
   décision produit à prendre quand viz atterrit.
