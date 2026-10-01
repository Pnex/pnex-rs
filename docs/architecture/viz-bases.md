# Bases de visualisation — carte, dashboards, parcours 3D, diagrammes (2026-09-11)

> PRD — trois « bases » interconnectées + un éditeur de diagrammes, posées
> sur les fondations existantes :
> - **Base map** — carte géographique à tuiles **et** plans d'étage par
>   bâtiment, avec **repères** (map pins) cliquables ;
> - **Base dashboard** — tableaux de bord composables de widgets métriques ;
> - **Base parcours 3D** — visite navigable multi-étages (V1 maquette
>   extrudée, V2 panoramas 360 greffés) ;
> - **Diagrammes** — éditeur libre type Excalidraw (formes, freehand,
>   flèches, texte) dont les objets peuvent porter des liens.
>
> Renvois : `media.md` (D21 — médias **référencés, jamais dupliqués**, couche
> 2/3 de la vision média), `flow-engine.md` (D18 — documents versionnés
> append-only), `inventory.md` (D2 org-tenant, D14 pagination, D13/D17 — la
> visualisation n'est jamais un chemin de contrôle).
> Contraintes transverses : Dioxus **CSR pur** (jamais fullstack/SSR),
> télémétrie = **REST polling** (pas de WS navigateur), endpoints **additifs**
> — ne jamais bumper `pnex_api_contract::CONTRACT`.
>
> ⚠ **Pivot POI-first (D35–D39, 2026-09-11)** : la hiérarchie
> site → bâtiment → étage de la Phase A est **retirée** (§0 bis) — le POI
> (`map_pins`) est l'objet primaire de la carte globale de l'org. Les
> sections décrivant sites/bâtiments/étages restent en l'état comme
> historique de conception ; le modèle livré fait foi.

## 0. Décisions actées (2026-09-11)

| # | Décision | Pourquoi |
|---|----------|----------|
| D22 | **Réutiliser `sites` tel quel, zéro ALTER** ; `svg_files`/`site_diagrams`/`annotations`/`saved_views` (migration 000004, dormantes sans contrôleur) sont **supplantés mais conservés** — aucun drop en 000014 | Migration purement additive et réversible ; le sort des tables dormantes attend l'audit de données (§9) |
| D23 | **Tables concrètes par base** + une seule table polymorphe **`viz_links`** (pas de table générique « viz_objects ») | Scoping org indexable (`org_id` dénormalisé), FK réelles vers `sites`/`floors`/`media_assets`, versioning aux sémantiques distinctes par base ; seule l'**arête de lien** est réellement polymorphe |
| D24 | **Documents versionnés école `flows`** : `dashboards/dashboard_versions`, `tours/tour_versions`, `viz_diagrams/viz_diagram_versions` (append-only, `expected_version_number` → 409, save = nouvelle version = live, pas de publish séparé en V1) ; les lignes (`sites`, `buildings`, `floors`, `map_pins`) restent un **état mutable simple** sans versioning | Précédent maison proué (`flows.rs` + `flow_editor/versions.rs`) ; le versioning n'a de sens que pour les documents composites |
| D25 | **Plans d'étage = `media_assets` kind `floorplan`** (image uploadée) **ou `viz_diagrams`** (plan dessiné) — jamais `svg_files` (SVG en TEXT en base viole la doctrine « octets jamais en base ») | `kind` est un string applicatif → extension **sans migration** ; octets en MediaStore (RustFS/fs) ; XOR imposé applicativement (`plan_media_asset_id` ⊕ `plan_diagram_id`, les deux NULL = étage sans plan) |
| D26 | **Liens stockés uniquement dans `viz_links`** (`source_kind/source_id/source_key` → `target_kind/target_id/target_ref`) ; org imposée à la création (cible résolue dans l'org, sinon 400) ; liens supprimés avec leur conteneur (app-level) ; **cible morte tolérée au rendu** (élément désactivé + toast, jamais panic) | Source de vérité unique — ni les pins, ni les widgets, ni les nœuds ne portent de colonne link ; la navigation croisée est un **graphe**, pas un workflow |
| D27 | **Carte géo = maplibre-gl 4.x (vectoriel)** consommant le style Protomaps **auto-hébergé** `https://map.alpine-box.com/style/light-en` (tuiles `map.alpine-box.com/basemap/{z}/{x}/{y}`, maxzoom 15) via IIFE `js/map.js` → `window.pnexMap` ; Leaflet raster écarté (ne consomme pas un style vectoriel) | Style de tuiles fourni par l'utilisateur (auto-hébergé = zéro quota OSM public, attribution Protomaps/OSM embarquée dans le style) ; **4.x épinglé pour WebGL1** (v5 exige WebGL2 — leçon Mali) ; repli legacy renderer à mesurer device (§9) |
| D28 | **Maquette 3D = three.js WebGL2** via IIFE `js/maquette.js` → `window.pnexMaquette`, **rendu à la demande** (pas de RAF continu), badge dégradé si WebGL2 absent | Précédent gsplat (WebGL2 + badge, D21) ; extrusion de plans + orbit = des semaines de WebGL à la main sinon |
| D29 | **Éditeur de diagrammes = SVG dioxus fait main** sur le squelette `flow_editor` ; **pas d'île React Excalidraw** ; compat `.excalidraw` par **sérialiseur Rust pur** import/export (sous-ensemble documenté) | Île React = ~500 Ko + double VDOM dans la même page + pont d'état bidirectionnel fragile ; le sous-ensemble (formes/arrows/freedraw/text/image) couvre le besoin |
| D30 | **Graphiques = SVG maison**, extension de `pages/visualisation.rs` extrait vers `components/charts/` (géométrie pure testée) | Zéro dépendance npm ; le chart maison existe déjà (polyline + grille) |
| D31 | **Valeurs live = polling REST existant au rendu** (`/telemetry/catalog`, `/telemetry/series`, `/dashboard/summary`) + **additif `POST /api/v1/telemetry/series-batch`** ; les séries ne sont **jamais stockées** ; rythme 15 s | Un seul timer par dashboard (Android) au lieu de N ; dégradation `available:false` **par item** ; un widget référence `(metric, device_id, window)` |
| D32 | **Navigation croisée = GlobalSignals typés** (`state/viz.rs`, école `OPEN_FLOW`) + **routes statiques à query params** ; les réponses de détail/overview **embarquent les liens résolus** | Props de route ≠ signaux (dioxus #2784) ; zéro requête réseau au clic ; deep-links partageables |
| D33 | **Migration unique additive `m20260911_000014_viz.rs`** — schéma viz complet d'un coup + `viz_saved_views` polymorphe (vues nommées sur les 4 bases) | École 000004 (un domaine = une migration) ; évite le churn de migrations par phase |
| D34 | **Modes live / édition** sur les 4 bases (`?mode=edit` → signal `VIZ_EDIT`, réservé `can_write()` — un viewer qui force le paramètre retombe en live) ; en édition, **polling suspendu + aperçu figé** (badge) ; déplacement d'un repère = **1 PATCH au pointer-up** (UI optimiste pendant le drag, jamais un PATCH par frame) | Le re-render live combat les gestes de l'éditeur (leçon Take 360 V3 : écritures DOM directes ~15 Hz, re-render dioxus réservé aux changements discrets) |
| D40 | **Canvas libre à main levée** pour les dashboards (studio SCADA, 2026-09-11) : widgets en **px absolus** dans un canvas dimensionné (`canvas {width,height,background}`) — la **grille** 12 cols initialement prévue est abandonnée avant toute implémentation ; **traits et accroches** (`wires`) purement **visuels** (liaison widget→widget, milieu des côtés, coude orthogonal, suit les déplacements, zéro sémantique d'exécution) | Demande user : « système de PID/graphique à main levée type SCADA » + « des traits et des accroches » ; un synoptique HMI se compose librement, pas en grille. **Numérotée D40 au merge** (D35–D39 = POI-first) |
| D41 | **Bibliothèque de widgets serveur** : table `viz_widget_library` (migration 000016, école sites UUID) — templates par org, **mutables sans versioning** (D24 : le versioning est pour les documents composites) ; l'instance posée **copie** le template (**snapshot au drag**) : éditer/supprimer un modèle n'affecte jamais les dashboards existants ; API `/api/v1/viz/widgets` + palette gauche + « enregistrer comme modèle » | Demande user (« bibliothèque modifiable à tout moment, pareil que les autres, versionnées ») — arbitrée : stockage serveur/org ; le versioning est porté par les dashboards eux-mêmes. **Numérotée D41 au merge** |

## 0 bis. Refonte POI-first — pivot utilisateur (2026-09-11, livré)

Retour utilisateur après la Phase A : la hiérarchie site → bâtiment → étage
« n'est pas la vision » — **on la retire**. Ce qui compte : voir **tous** les
POIs sur une carte globale, pouvoir en créer n'importe où sans cérémonie,
attacher des objets réels (devices, panoramas 360°) et préparer le futur
**geofencing / tracking GPS**.

| # | Décision | Pourquoi |
|---|----------|----------|
| D35 | **Hiérarchie supprimée** : migration **destructrice** `m20260911_000015_poi_first.rs` — drop `floors`, `buildings`, `tour_versions`, `tours` (000014) et `annotations`, `saved_views`, `site_diagrams`, `svg_files`, `sites` (000004) ; `map_pins` perd `site_id`/`floor_id`, gagne `location_detail` varchar(255) (localisation **libre** « Bât. A — Étage 2 ») ; `mode`/`x`/`y` et le CHECK geo/plan **conservés** pour les plans futurs sans hiérarchie | L'app n'a jamais été releasée, données = seuls tests dev ; le POI devient l'objet primaire ; **D22/D23 abrogées**, D25 suspendue (plans réancrés plus tard) |
| D36 | **Un device = un seul POI** : index unique **partiel** `uniq_map_pins_org_device (org_id, device_id) WHERE device_id IS NOT NULL` + garde applicative (sqlite) ; `device_id` validé contre `device_registries` de l'org (400 `device_id` inconnu / déjà placé) ; détachement = PATCH `device_id: null` | « Si un device est attaché à un POI, il ne pourra pas être attaché sur une autre adresse » — la position d'un IoT physique est unique |
| D37 | **Clustering backend** : `GET /api/v1/pois/cluster?bbox=west,south,east,north&zoom=…` — grille server-side `cell = 360/(8×2^zoom)` (≈64 px, tuiles 512), centroïde par cellule ; ≤ 200 points dans la bbox → **points individuels** (id/label/emoji), sinon **clusters numérotés** (count + centroïde + échantillon) ; cap 500 items (cellule doublée) ; filtres list applicables ; le front refetch à chaque `moveend` debouncé (300 ms) et zoome (+2) au clic cluster | « Éviter de polluer et d'avoir 500 points directement affichés » — jamais plus de quelques centaines de markers DOM, quels que soient les données |
| D38 | **GPS devices** : convention télémétrie **`latitude`/`longitude`** (WGS84 degrés décimaux, métriques normales) + optionnels `gps_accuracy_m`, `gps_altitude_m`, `gps_speed_mps`, `gps_heading_deg` ; table `device_positions` (**dernière** position par device — upsert, `device_registry_id` UNIQUE) alimentée par un **tap sur le sink télémétrie** (`GpsTapSink`, jamais bloquant, erreurs loggées) ; `source` = `telemetry|manual` (PUT manuel POC/tests) ; **historique complet = O2** (les métriques GPS y sont déjà des séries) ; couche carte live pollée 15 s + filtre « avec position GPS » | Penser global dès le POC : geofencing (zones) et tracking (trails via `query_range` O2) consommeront cette table/séries — hors scope V1 |
| D39 | **Attachements** : device = colonne `map_pins.device_id` (D36) ; **panoramas 360° et futurs dashboards/diagrammes = `viz_links`** (D26) — premier contrôleur d'arêtes : `GET|POST /api/v1/viz/links`, `DELETE /api/v1/viz/links/{id}` ; source `map_pin`, cibles `media_asset|dashboard|viz_diagram` à existence org validée ; cascade sources déjà en place (`delete_links_for`) | Les liens restent un registre unique ; le POC « attacher des devices, des vues 360° » passe par là |
> ℹ **D42 (2026-09-12)** : `viz_links` est **absorbé** par la couche d'organisation
> transverse (`resource_edges`, relation `placed_on`, placement porté par l'arête,
> cascade symétrique). Les endpoints `/api/v1/viz/links` restent des thin-wrappers ;
> les nouvelles intégrations passent par `/api/v1/resources/edges`.
> ℹ **D43 (2026-09-12)** : POI **multi-devices** — table `device_placements`
> (migration 000019) : **N devices par repère**, **1 placement par device**
> (UNIQUE `device_registry_id`, PG **et** sqlite), `location_detail` par
> placement (« Rack 3 — Allée B ») en plus du `location_detail` bâtiment du
> POI. Attacher un device déjà placé répond **409** porteur du POI courant,
> le déplacement est un PATCH délibéré `pin_id` après confirmation UI.
> D36 **supplantée** (colonne `map_pins.device_id` + index partiel droppés) ;
> D39/D42 inchangés (média/tour/dashboard restent des arêtes `placed_on`
> many-to-many, réutilisables sur plusieurs POIs). Contrat :
> `pnex_api_contract::CONTRACT` → 2.
> ℹ **Amendement D43 (2026-09-13) — détachement** : les devices sont
> créés libres et s'attachent/détachent librement ; **seule contrainte :
> l'unicité** (1 device ≤ 1 placement). `DELETE /api/v1/pois/placements/{id}`
> (204, 404 masqué cross-org, 403 viewer) rend le device plaçable n'importe
> où ; le bouton **Détacher** du drawer (/map) couvre les devices racine
> (placement parti) et les devices rangés (éjection du dossier, placement
> intact). Le 409/move reste le chemin du re-positionnement délibéré.

Endpoints livrés (voir `docs/contracts/viz.http`) : `GET|POST /api/v1/pois`,
`GET|PATCH|DELETE /api/v1/pois/{id}`, `GET /api/v1/pois/cluster`,
`POST /api/v1/pois/{id}/devices`,
`PATCH|DELETE /api/v1/pois/placements/{id}` (DELETE = détachement,
amendement D43),
`GET|POST /api/v1/viz/links`, `DELETE /api/v1/viz/links/{id}`,
`GET /api/v1/device-positions`, `PUT /api/v1/device-positions/{device_id}`.

UI livrée (`pages/map.rs`, plein écran sous la shell) : **sidebar gauche
repliable** (recherche, filtres device/GPS/emoji, « Tout afficher », liste
paginée), bouton flottant **« ＋ Ajouter un POI »** (le prochain clic carte
ouvre le formulaire pré-rempli, palette 12 emoji + champ emoji libre),
drawer de détail (édition, recentrage, **sélecteur de device attaché**,
panoramas liés avec viewer pannellum en modale, suppression confirmée),
**couche GPS 🛰️** distincte. Front : `js/map.js` expose `setItems`
(pins/clusters/positions) + événements `moveend`/clic carte nue
(`window.__pnexMapView` / `__pnexMapLastClick`), pont `map_viewer.rs`
(`MapItem`, `take_viewport`, `flyTo`).

Tests : `tests/pois.rs` (6 — CRUD/D14, unicité device + hardening lat/lon/
emoji, cluster avec grille de 205 POIs, liens + cascade, positions observe/
manuel + `has_position`, isolation org + viewer 403) et 5 unitaires purs du
bucketing (`services/pois.rs::tests`).

## 1. Périmètre & objets

Quatre objets authored par org (D2), tous accessibles en **live** (consultation)
et en **édition** (D34) :

| Objet | Table | Contenu | Éditeur |
|---|---|---|---|
| **Site** | `sites` (existant) | ancre géo (lat/lon), adresse, zoom par défaut | formulaire (modal) |
| **Bâtiment / étages** | `buildings` / `floors` | empreinte geo, hauteur maquette ; étages ordonnés (`level`), chacun avec son plan (image XOR dessiné) | éditeur de plan (squelette flow_editor) |
| **Repère** (map pin) | `map_pins` | position géo **ou** plan, device lié (lien faible), lien(s) sortant(s) | posé dans l'éditeur de plan / la carte |
| **Dashboard** | `dashboards(+_versions)` + `viz_widget_library` (D41) | canvas libre à main levée (D40) : widgets `(metric, device_id, window)` en px absolus + traits/accroches visuels + options ; bibliothèque de templates par org | studio SCADA (`dashboard_editor`) |
| **Parcours 3D** | `tours(+_versions)` | nœuds (`floor` V1 maquette, `panorama` V2), caméras, hotspots | éditeur de tour |
| **Diagramme** | `viz_diagrams(+_versions)` | éléments type Excalidraw, dont éléments porteurs de liens | éditeur de diagrammes |

**Glossaire UI** (i18n soigné) : **repère** = `map_pins` (objet cartographique) —
à ne pas confondre avec les **broches** GPIO (`controllers/pins.rs`, page Pins
existante) ; « bâtiment »/« étage » ; « parcours » = tour 3D.

**Non-buts** (reste hors périmètre) : indoor positioning / localisation temps
réel ; co-édition temps réel (concurrence optimiste 409 suffit) ; tout chemin
de contrôle serveur (frontières D13/D17 — un lien « nœud 3D → actionneur »
n'existera pas) ; géo↔plan automatique (correspondance device carte ↔ étage,
§9) ; rendu natif des previews lourdes (D21 V1 s'applique).

## 2. Modèle de données

Migration `crates/pnex-backend/migration/src/m20260911_000014_viz.rs`
(enregistrée dans `migration/src/lib.rs` ; prochaine libre = **000014**).
École `sites` : PK UUID (`uuid_pk()` — `gen_random_uuid()` PG-only), `org_id`
BIGINT NOT NULL FK→organizations CASCADE **dénormalisé sur chaque table**,
JSONB, codes string (pas d'enum PG), index `uniq_/idx_` via
`execute_unprepared` (un statement par appel, sqlite-portable),
`created_at/updated_at`.

**`buildings`** — `id` UUID PK, `org_id` NN, `site_id` UUID NN FK→sites
CASCADE, `name` varchar(255) NN, `description` TEXT NULL,
`footprint` JSONB NULL (polygone `[[lat,lon],…]` — carte géo + base
d'extrusion 3D), `height_m` decimal(6,2) NULL (hauteur maquette),
`address` TEXT NULL, `tags`/`metadata` JSONB NULL.
Index `(org_id, site_id, name)`.

**`floors`** — `id` UUID PK, `org_id` NN, `building_id` UUID NN FK→buildings
CASCADE, `name` varchar(255) NN, `level` INT NN (0 = RDC, négatif = sous-sol,
**UNIQUE `(building_id, level)`**), `display_order` INT NN DEFAULT 0,
`plan_media_asset_id` UUID NULL FK→media_assets **ON DELETE SET NULL** (plan
image), `plan_diagram_id` UUID NULL FK→viz_diagrams ON DELETE SET NULL (plan
dessiné), `plan_width`/`plan_height` INT NULL (dimensions natives px),
`plan_scale` decimal(10,4) NULL (mètres par pixel), `metadata` JSONB NULL.
**Contrainte XOR applicative** : `plan_media_asset_id` ⊕ `plan_diagram_id`
(les deux NULL = étage sans plan). Index `(org_id, building_id, display_order)`.

**`map_pins`** — le **POI** (point d'intérêt) : `id` UUID PK, `org_id` NN,
`site_id` UUID NN FK→sites CASCADE, `floor_id` UUID NULL FK→floors CASCADE
(**NULL = POI géo outdoor au niveau site**), `mode` varchar(16) NN
(`geo|plan`), `latitude`/`longitude` decimal(9,6) NULL, `x`/`y`
decimal(10,2) NULL (unités du plan), `device_id` varchar(255) NULL (**lien
faible sans FK** = le **slug** `device_registries.device_id`, tel qu'utilisé
par `/telemetry/*` — école `annotations.linked_devices` ; **facultatif** :
un POI n'est pas forcément un device), `label` varchar(255) NN, `emoji`
varchar(16) NULL (**pictogramme du POI** — un graphème emoji, défaut `📍` ;
rendu en texte, aucun asset image), `metadata` JSONB NULL.
CHECK exclusif : `geo` ⇒ lat/lon NOT NULL ∧ x/y NULL ; `plan` ⇒ x/y NOT NULL
∧ lat/lon NULL. Index `(org_id, site_id)` + `(floor_id)`. Un POI « libre »
(sans device) est un cas ordinaire, pas une exception.

**`device_placements`** (D43, migration 000019 — remplace la colonne
`map_pins.device_id` droppée) : `id` BIGSERIAL PK, `org_id` NN FK CASCADE,
`device_registry_id` BIGINT NN **UNIQUE** FK CASCADE (1 placement par
device), `device_id` varchar(255) NN (slug dénormalisé, école
`device_positions`), `pin_id` UUID NN FK→`map_pins` CASCADE, `location_detail`
varchar(255) NULL (localisation fine **par device**, dans le site), index
`(org_id, pin_id)`. Détachement possible depuis l'amendement D43
(2026-09-13) : le device redevient libre, l'UNIQUE garantit toujours
« au plus un POI à la fois ».

**`dashboards` / `dashboard_versions`** (école `flows`) —
`dashboards` : `id` UUID PK, `org_id` NN, `name` NN, `description` NULL,
`current_version_number` BIGINT NN DEFAULT 0, `tags`/`metadata` JSONB.
`dashboard_versions` : `id` UUID PK, `dashboard_id` UUID NN FK CASCADE,
`version_number` BIGINT NN (UNIQUE `(dashboard_id, version_number)`),
`layout` JSONB NN, `created_by` BIGINT NULL, `created_at`.

**`tours` / `tour_versions`** — même forme ; en plus sur `tours` :
`site_id` UUID NN FK→sites CASCADE, `mode` varchar(16) NN DEFAULT `maquette`
(`maquette|panorama` — colonne de filtrage de liste, tenue à jour au save).
`tour_versions.doc` JSONB NN = les nœuds du parcours.

**`viz_diagrams` / `viz_diagram_versions`** — même forme que dashboards ;
`viz_diagram_versions.elements` JSONB NN.

**`viz_links`** — le registre de liens typés :

| Colonne | Type | Rôle |
|---|---|---|
| `id` | UUID PK | — |
| `org_id` | BIGINT NN FK CASCADE | scoping D2 |
| `source_kind` | varchar(32) NN | `site`, `building`, `floor`, `map_pin`, `dashboard`, `dashboard_widget`, `tour`, `tour_node`, `diagram`, `diagram_element` |
| `source_id` | UUID NN | id du **conteneur** (pour un sous-objet : id du document) |
| `source_key` | varchar(128) NULL | id de l'élément/widget/nœud **dans** le document (UUID généré à l'auteur, stable) |
| `target_kind` | varchar(32) NN | `site`, `floor`, `dashboard`, `tour`, `diagram`, `metric`, `device`, `media_asset` |
| `target_id` | UUID NULL | NULL pour `metric` |
| `target_ref` | JSONB NULL | `{"metric":…,"device_id":…,"window":…}` ou `{"url":…}` |
| `label`, `metadata` | varchar/JSONB NULL | affichage |
| `created_at/updated_at` | timestamptz | — |

Index `(org_id, source_kind, source_id)` + `(org_id, target_kind, target_id)`.
CHECKs app-level : `metric` **et** `device` ⇒ `target_id` NULL ∧
`target_ref` complet (`{"metric":…}` / `{"device_id":"<slug>"}` — les ids
device du repo sont des **slugs** string, PK i64 non exposée) ; pour tout
autre target_kind, `target_id` NOT NULL. Règles D26 : unicité source (kind,id,key) →
cible autorisée en multiple ? Non — **un lien = une arête**, recréer écrase
(upsert par source) ; suppression du conteneur ⇒ suppression des liens
(app-level, pas de FK polymorphe).

**`viz_saved_views`** — `id` UUID PK, `org_id` NN, `target_kind` varchar(32)
NN (`site|floor|dashboard|tour|diagram`), `target_id` UUID NN, `name` NN,
`state` JSONB NN (`{"zoom":1.5,"pan_x":120,"pan_y":-40,"floor_id":null,
"node_id":"n1"}`), `tags` JSONB NULL. Index `(org_id, target_kind, target_id)`.

### Schémas JSON des documents

**`dashboard_versions.layout`** — **canvas libre** (D40) : widgets en px
absolus + traits/accroches visuels ; un widget référence des séries par
rôle (`source` = liste) ; les points ne sont **jamais** stockés (D31) :

```json
{
  "canvas": {"width": 1600, "height": 900, "background": "#f8fafc"},
  "widgets": [
    {
      "id": "w-0001",
      "type": "gauge | stat | line | indicator | text",
      "title": "Température serveur",
      "x": 120, "y": 80, "w": 240, "h": 200,
      "source": [
        {"role": "primary", "metric": "temperature",
         "device_id": "soil-01", "window": "1h"}
      ],
      "options": {"unit": "°C", "min": 0, "max": 50,
                   "decimals": 1, "thresholds": [{"value": 40, "color": "#dc2626"}],
                   "text": "… (widget text)"}
    }
  ],
  "wires": [
    {"id": "t-0001",
     "from": {"widget_id": "w-0001", "side": "right"},
     "to": {"widget_id": "w-0002", "side": "left"}}
  ]
}
```

**`tour_versions.doc`** — V1 `maquette` (nœuds `floor`), V2 `panorama`
(nœuds `media_asset_id` — **référence** `media_assets`, jamais dupliqué, D21) :

```json
{
  "mode": "maquette",
  "start_node": "n1",
  "nodes": [
    {
      "id": "n1", "kind": "floor", "floor_id": "0197…",
      "label": "RDC",
      "camera": {"yaw": 0, "pitch": -30, "distance": 40,
                 "target": [10.0, 5.0]},
      "extrusion": {"height_m": 3.2, "base_color": "#94a3b8",
                    "wall_opacity": 0.85},
      "hotspots": [{"id": "h1", "to": "n2",
                    "position": [10, 5, 0], "label": "Monter au 1er"}]
    }
  ]
}
```

**`viz_diagram_versions.elements`** — sous-ensemble type Excalidraw, forme
maîtresse maison (l'import `.excalidraw` **normalise** vers cette forme) :

```json
{
  "app_state": {"grid_size": 20, "background": "#ffffff"},
  "elements": [
    {
      "id": "e1-uuid",
      "type": "rectangle | ellipse | diamond | arrow | line | freedraw | text | image",
      "x": 0, "y": 0, "width": 120, "height": 60, "angle": 0,
      "stroke_color": "#1e293b", "background_color": "#f8fafc",
      "fill": "hachure | solid | none", "stroke_width": 2, "roughness": 1,
      "points": [[0, 0], [120, 60]],
      "text": "", "font_size": 16, "text_align": "left",
      "media_asset_id": null, "z_index": 3
    }
  ]
}
```

Enums **ouvertes** : `widgets[].type` et `elements[].type` sont des strings
validés par liste côté service — un nouveau type (ex. widget psychrométrique
Mollier, §9) s'ajoute **sans migration**.

## 3. API

Cinq contrôleurs additifs (`crates/pnex-backend/src/controllers/`, registés
dans `app.rs`), tous sous `OrgContext` : lectures owner|admin|viewer,
écritures `can_write()` ; 400 champ par champ `{"<champ>": msg}` ; 404 masqué
cross-org ; **listes = enveloppe D14** `{count, next, previous, results}` +
`limit/offset/search`. Contrat : `docs/contracts/viz.http`. Endpoints
**additifs — ne jamais bumper `CONTRACT`**.

| Contrôleur | Préfixe | Endpoints |
|---|---|---|
| `sites.rs` | `/api/v1/sites` | `GET ""` (D14, search name/address), `POST ""`, `GET/PATCH/DELETE /{id}` · **`GET /{id}/overview`** (site + bâtiments + étages + repères + liens résolus en une requête — école `dashboard/summary` : une requête = un timer) · `GET/POST /{id}/buildings`, `PATCH/DELETE /buildings/{id}` · `GET /{id}/floors?building_id=`, `POST /{id}/floors`, `PATCH/DELETE /floors/{id}` · `GET/POST /{id}/pins?floor_id=`, `PATCH/DELETE /pins/{id}` |
| `dashboards.rs` | `/api/v1/dashboards` | `GET ""`, `POST ""` · `GET /{id}` · **`PATCH /{id}` = save** (`expected_version_number` → **409** école flows) · `DELETE /{id}` · `GET /{id}/versions`, `GET /{id}/versions/{n}`, `POST /{id}/versions/{n}/restore` (école media) |
| `tours.rs` | `/api/v1/tours` | même forme que dashboards (+ filtre liste `?site_id=&mode=`) |
| `diagrams.rs` | `/api/v1/diagrams` | même forme que dashboards |
| `viz_links.rs` | `/api/v1/viz/links` | `GET ""` (`?source_kind=&source_id=` ou `?target_kind=&target_id=`, D14), `POST ""`, `PATCH/DELETE /{id}` · `POST /validate` (batch `(kind,id)[]` → existence par org ; pour le panneau de liens de l'éditeur) |

**Plans d'étage — aucun nouvel endpoint d'upload** : l'image part par
`POST /api/v1/media` existant (octet-stream) avec le nouveau kind
applicatif **`floorplan`** (validation par liste étendue côté service —
`kind` est un string, pas de migration), puis `PATCH /api/v1/sites/floors/{id}`
`{"plan_media_asset_id": …}`. Les plans SVG dessinés passent par
`viz_diagrams` (§2).

**Additif télémétrie** : `POST /api/v1/telemetry/series-batch` — corps
`{"specs": [{"metric","device_id","window"}…]}`, réponse
`{"results": [<même forme que /series>]…}` avec dégradation **par item**
(`available:false`). Étend `controllers/visualization.rs` +
`services/visualization.rs` (anti-injection PromQL inchangée : mêmes
validations de charset/presets par spec).

**Valeurs live** (repères, widgets, nœuds de tour) : résolues **au rendu**
par polling des endpoints existants — jamais stockées, jamais calculées
côté serveur dans une boucle (D13/D17).

⚠ **Proximité de préfixes à documenter** : `/api/v1/dashboard` (existant,
résumé télémétrie, **singulier**) vs `/api/v1/dashboards` (nouveau, CRUD
des tableaux de bord, **pluriel**). Distincts et additifs ; les libellés UI
disambiguent (« Tableaux de bord » vs « Visualisation »).

## 4. Stockage

- Octets **jamais en base** (doctrine D21) — plans d'étage et panoramas de
  tour = `media_assets`/`media_versions` existants, MediaStore opendal
  (`fs` défaut / `s3` RustFS), clés `org_{org}/media/{asset}/{version}_{filename}`.
- Lecture = endpoints `/content` existants (streaming authentifié, **pas
  d'URLs présignées**) ; le front fabrique ses blob URLs (web) / data-URIs
  (natif) via `util::media_blob_url` — pattern `media_viewer.rs` inchangé.
- **Règle plans ≤ ~2 MP** : côté client, downscale/refus à l'upload — les
  data-URIs Android sont un risque mémoire (§5) ; les panoramas restent
  dans la limite `media.max_bytes` existante.
- Nouveau kind applicatif `floorplan` dans la liste de validation
  (`photo|panorama|splat|floorplan`) — sniff GPano **ne** reclassifie **pas**
  un floorplan (kind client prioritaire, règle existante).

## 5. UI / Frontend

Dioxus 0.7 CSR pur, routes **statiques** uniquement (dioxus #2784 : les props
de route ne sont pas des signaux) ; deep-links par query params hydratant des
GlobalSignals. Nouvelles routes dans `src/app.rs` (layout `Shell`) :

```
#[route("/map?:site&:floor&:mode")]     Map { site, floor, mode }
#[route("/dashboards?:id&:mode")]       Dashboards { id, mode }
#[route("/tours?:id&:node&:mode")]      Tours { id, node, mode }
#[route("/diagrams?:id&:mode")]         Diagrams { id, mode }
```

Nouveaux fichiers :

- `src/state/viz.rs` — `OPEN_SITE/OPEN_FLOOR/OPEN_DASHBOARD/OPEN_TOUR/
  OPEN_DIAGRAM`, `VIZ_EDIT`, enum `VizTarget`, `open(target)` (école
  `state/flows.rs::OPEN_FLOW`) ;
- `src/api/viz.rs` — client des 5 contrôleurs ;
- `src/pages/{map,dashboards,tours,diagrams}.rs` — liste + vue (signal
  `selected`, idiome `reload`/`use_resource`/polling 15 s) ;
- `src/components/viz/{link.rs, bridge.rs, loader.rs, canvas_math.rs}` —
  résolveur de liens, pont JS↔Rust, loader de blobs, math canvas unifiée ;
- `src/components/map_editor/`, `diagram_editor/`, `dashboard_editor/` —
  squelette `flow_editor` (mod/canvas/geometry/state/inspector) ;
- `src/components/charts/` — extraction du chart SVG de
  `pages/visualisation.rs` (régression visuelle : rendu identique).

### Patterns réutilisés (tous éprouvés)

| Besoin | Pattern source | Réutilisation |
|---|---|---|
| Canvas éditable (plan, diagramme, dashboard) | `components/flow_editor/` : SVG sans viewBox (1 unité = 1 px), `geometry.rs` pur testé, reducers purs `state.rs`, machine `Interaction`, `EditorCx`, inspector, versions drawer | squelette recopié, modèle remplacé (shapes/strokes au lieu de nodes/wires) ; undo/redo **client** ajouté (absent du flow editor) |
| Gestes sur natif | `geometry.rs::canvas_rect()` renvoie `None` hors web → **`viz/canvas_math.rs` unifié web-sys** (une seule branche, wry a un vrai DOM) + fallback par geste + **spot check device en DoD** ; `flow_editor` lui-même non touché en V1 | corrige le gap natif pour les nouveaux éditeurs |
| Pont JS → Rust | `flash.rs` : `Closure::wrap` passé à `window.pnex*.watch(cb)`, décodage serde en enum d'events, stub cfg natif | `pnexMap.watch` / `pnexMaquette.watch` |
| Mount/unmount viewer | `media_viewer.rs` : IIFE `window.pnex*`, resolve `js_sys::Reflect` (wasm) / `document::eval` (natif), retry, échec → **badge, jamais panic**, `use_effect`/`use_drop` | `pnexMap`, `pnexMaquette` |
| Octets authentifiés | `util::media_blob_url` (blob URL web, data-URI natif) | plans, textures, panoramas |
| Perf continue | leçon Take 360 V3 : écritures DOM directes ~15 Hz (`setPinValue(id, html)`), re-render dioxus **discret** uniquement ; three.js rendu **à la demande** | repères live, maquette |

### Bibliothèques (bundles esbuild IIFE, école `js/viewers.js`)

| Bundle | Contenu | Global | Charge |
|---|---|---|---|
| `js/map.js` (+ CSS) | maplibre-gl **4.x** (WebGL1-compatible) + style Protomaps auto-hébergé (`map.alpine-box.com/style/light-en`) + couche POI/repères (source GeoJSON, logo custom par `icon-image`) | `window.pnexMap` | `<script>` statique dans `main.rs`, école flasher/viewers |
| `js/maquette.js` | three.js (extrusion des plans, orbit, hotspots) | `window.pnexMaquette` | idem ; ~1 Mo dans l'APK, **acceptable V1** (lazy-loading en backlog) |

Éditeurs = **aucune** lib (SVG dioxus + modules purs). Charts = **aucune**
lib. npm : ajout de `maplibre-gl` (4.x) et `three` épinglés + scripts
`js:map` / `js:maquette` dans `package.json`, extension `js:build`/`js:ensure`
et `sources` du Taskfile.

### Modes live / édition (D34)

- `mode=edit` hydrate `VIZ_EDIT` ; si `role` viewer → retombe en live
  (boutons d'édition masqués, l'API renverrait 403 de toute façon).
- **Live** : polling 15 s (widgets via `series-batch`, repères via
  `setPinValue` en écriture DOM directe, dégradé `available:false` par item
  → badge gris + tooltip).
- **Édition** : polling **suspendu**, dernière valeur figée + badge
  `t!("viz-preview-frozen")` ; save = PATCH avec `expected_version_number`
  → modale de conflit 409 (recharger / écraser), école `flow_editor` ;
  repères/floors = dernier écrit gagne (pas de versioning, D24).

### Conventions transverses

Tailwind v4 classes littérales ; **toute** chaîne visible via `t!` (Fluent
`fr-FR`/`en-US`, parité testée — registre `viz-*`) ; toasts = erreurs
relayées, jamais traduites ; widgets réutilisés (`Modal`, drawer droit,
`Pager`, `EmptyState`). **Pictogramme POI = emoji** : palette maison
curatée (grid de ~100 emoji usuels — capteurs, eau, énergie, accès,
risque… posée dans un `Popover`) sur desktop ; sur mobile le champ accepte
le clavier emoji natif — zéro dépendance, l'emoji est rendu comme du texte
dans le marker maplibre (`icon-image` inutile : layer `symbol` avec
`text-field` = emoji + police emoji du système).

## 6. Navigation croisée

- **`VizTarget`** (`state/viz.rs`) : `Site(Uuid)`, `Floor{site, floor}`,
  `Dashboard(Uuid)`, `Tour{id, node}`, `Diagram{id, element}`,
  `Metric{metric, device_id, window}`, `Device(String)` (slug) —
  `open(target)` pose le signal + `navigator().push(...)` (école
  `OPEN_FLOW`).
- **Résolution sans requête au clic** : `GET /sites/{id}/overview` embarque
  les liens résolus de chaque repère (cible typée) ; le détail d'un dashboard
  embarque les liens de widgets ; le fetch d'un `tour_versions.doc` embarque
  les liens de nœuds. Fallback : `GET /viz/links/{id}`.
- `components/viz/link.rs` rend le hotspot cliquable et appelle `open()` ;
  cible morte (404 masqué) → toast `t!("viz-link-unavailable")` + élément
  rendu désactivé — **jamais de panic**.
- **Cycles tolérés** (c'est un graphe de navigation) ; garde-fous :
  pas d'auto-follow au-delà de la cible d'entrée ; breadcrumb des 5 dernières
  cibles (dédupliqué) dans le Shell ; back/forward navigateur ; l'éditeur de
  liens interdit le self-target (même `source_id`+`source_key`) et la boucle
  sur le même conteneur.
- Cible `Metric` → ouvre `/visualisation` pré-remplie (metric/device/window) ;
  cible `Device` → page Devices du device.

## 7. Tests

Backend (école `tests/common/mod.rs` : mock JWKS Rauthy, `TEST_DATABASE_URL`
PostgreSQL, `serial_test`) :

- `tests/sites.rs` — CRUD site/building/floor/pin, CHECKs geo/plan, XOR
  plan, overview agrégé, 404 masqué cross-org ;
- `tests/dashboards.rs`, `tests/tours.rs`, `tests/diagrams.rs` — CRUD +
  versions append-only + 409 `expected_version_number` + restore ;
- `tests/viz_links.rs` — création (cible inconnue → 400 `{"target": …}`),
  org imposée, suppression en cascade avec le conteneur, `POST /validate` ;
- extension `tests/tenant_isolation.rs` (alice/bob sur chaque nouveau
  contrôleur) et `tests/visualization.rs` (series-batch : batch valide,
  spec invalide isolée, O2 down → `available:false` par item).

Frontend (unitaires purs, wasm32) : `geometry`/`canvas_math` (transforms,
hit tests, snap, zoom-toward-cursor), reducers d'éditeurs, undo/redo,
**sérialiseur `.excalidraw`** (fixtures import→export round-trip sur le
sous-ensemble documenté), composants `charts` (génération de paths).
Parité i18n fr/en (test existant `i18n::tests::parite_cles_fr_en`).

Manuel/scripté : matrice web + Android (émulateur `Medium_Phone_API_36.1`,
`10.0.2.2:5150`) par phase — spot checks chromium headless pour les viewers
JS (école : « mount OK + rendu réel » de `viewers.js`) ; `task check`
(natif + wasm32) / `task test` / `task lint` verts avant chaque commit
(« jamais commit red »).

## 8. Definition of Done (par phase)

Chaque phase : `task check` (natif + wasm32) + `task test` + `task lint`
verts, parité i18n, **zéro bump `CONTRACT`**, Android buildable
(`task build:frontend:android`).

- **Phase A — Fondations carte & étages** : migration `000014` complète,
  `services/sites.rs` + `controllers/sites.rs` + tests ; viewer `/map`
  (Leaflet : site, bâtiments, étages, plans image, repères), kind
  `floorplan` accepté. **Fini quand** : un site avec bâtiments/étages/repères
  s'affiche et se navigue web + Android ; listes D14 ; repère cliquable
  journalise sa cible (nav croisée pas encore).
- **Phase B — Dashboards + télémétrie** : `dashboards.rs` (versions + 409) +
  tests ; `series-batch` ; extraction `components/charts/` (rendu identique) ;
  `pages/dashboards.rs` + `dashboard_editor` (**canvas libre D40**, bibliothèque
  D41 : migration 000016 + `/api/v1/viz/widgets`, picker branché sur
  `/telemetry/catalog`). **LIVRÉE le 2026-09-11** (commits ce6e62f + e3415e1)
  — **cross-nav v1 repoussée** (`viz_links` POST/GET/DELETE + résolveur :
  repère → dashboard reste à la suite de la Phase B). **Fini quand (atteint)** :
  un dashboard compose des widgets à main levée depuis la bibliothèque,
  s'enregistre (versions + 409), se restore, et s'affiche **live** avec un
  chemin dégradé propre (dégradation par item du batch).
- **Phase C — Éditeur de diagrammes** : `diagrams.rs` + versions + tests ;
  `diagram_editor` (squelette flow_editor), `excalidraw.rs` import/export +
  fixtures ; outil lien (créer un `viz_links` depuis la sélection) ;
  `canvas_math.rs` unifié (correctif natif) **vérifié sur device**.
  **Fini quand** : créer/éditer/enregistrer avec modale 409 + historique ;
  un `.excalidraw` de fixture importe puis ré-exporte sans perte sur le
  sous-ensemble ; un élément de diagramme ouvre un dashboard.
- **Phase D — Parcours 3D V1 (maquette)** : `tours.rs` + tests ; player
  `/tours` (`pnexMaquette` : extrusion des floors, hotspots, rendu à la
  demande, badge WebGL2) ; `tour_editor` (nœuds, caméras) ; liens nœud →
  dashboard/métrique. **Fini quand** : une visite V1 se parcourt clavier +
  tactile sur web et Android ; hotspot → dashboard fonctionne.
- **Phase E — Cross-nav complète, vues sauvegardées, V2 panoramas** :
  `viz_links` full CRUD + `/validate` + breadcrumb ; `viz_saved_views` ;
  tours V2 nœuds `panorama` (pannellum dans les hotspots, patterns D21) ;
  décision finale tables dormantes après audit ; registres à jour.
  **Fini quand** : les trois bases sont interliées dans les deux sens ; un
  nœud panorama joue un 360° stitché existant ; docs à jour.

## 9. Reste ouvert

1. **Sort des tables dormantes** (`sites`/`svg_files`/`site_diagrams`/
   `annotations`/`saved_views`) : conservation actée en 000014 (D22) ; un
   **drop** a été proposé à l'étude de design — trancher après
   `SELECT count(*)` sur les PG de dev (lignes de test éventuelles) ; si
   drop, migration de nettoyage séparée, jamais dans 000014.
2. **Style de tuiles** : URL configurable (constante/env du front, défaut
   `https://map.alpine-box.com/style/light-en` — Protomaps auto-hébergé) ;
   repli vieux webviews sans WebGL1 à mesurer device (maplibre legacy
   renderer ou dégradé « pas de carte géo ») ; l'attribution Protomaps/OSM
   embarquée dans le style doit rester visible.
3. **Fidélité `.excalidraw`** : sous-ensemble documenté (formes, arrows,
   lines, freedraw, text ; liaisons flèche↔élément, frames et laser
   exclus) — import **lossy-normalizing** assumé.
4. **Widget psychrométrique / Mollier (+ CoolProp)** : extension future —
   les enums ouvertes `widgets[].type`/`elements[].type` le permettent sans
   migration ; source de propriétés à trancher (CoolProp→wasm vs endpoint
   serveur, école D1/FastAPI).
5. **Lazy-loading des bundles** (`maquette.js` ~1 Mo) — optimisation,
   pas V1.
6. **`tours.mode` colonne vs doc** : colonne de filtrage tenue à jour au
   save — drift possible accepté et documenté.
7. **URL `/content` tokenisée** pour textures lourdes (alternative aux
   data-URIs Android) — implications sécurité à étudier.
8. **Correspondance géo↔plan d'un même device** (repère carte + repères
   étages) : autorisé par le schéma, pas d'UI dédiée V1.
9. ~~**POI sans site**~~ **résolu par la refonte POI-first (D35)** : la
   hiérarchie sites a été supprimée (migration 000015) ; la question ne se
   pose plus — tout POI est directement sur la carte globale de l'org.

## 10. Roadmap

Phases A → E (§8) sur branche dédiée par phase, dans l'ordre — backend
additif d'abord (A), socle dashboard/métriques qui réutilise l'existant (B),
éditeur de diagrammes indépendant (C), 3D (D), interconnexion complète et
panoramas (E). Backlog post-E : Mollier/CoolProp (§9.4), lazy bundles
(§9.5), URL tokenisée (§9.7), unification `canvas_math` de `flow_editor`.

⚠ Réancrage POI-first (D35) : la Phase B (dashboards) est inchangée ; la
Phase D recréera `tours`/`tour_versions` **sans** `site_id` (pivot : POI ou
org comme ancre) ; les plans d'étage (D25) reviendront, s'ils reviennent,
**sans** buildings/floors — plan référencé directement par un POI ou un
groupe de POIs.
