# Annotations sur médias — couches versionnées posées sur les tours (D55–D60)

> **Statut : DOCTRINE (réflexion tranchée le 2026-09-13, implémentation par
> phases — §8).** Référence pour tout le chantier annotations : ancrage
> média (D55), versioning + activation (D56), cibles faibles (D57),
> rendu (D58), édition dans le tour (D59), portes ouvertes (D60).
> **Révision D147 (2026-10-07, §13)** : mini-dashboards sur le média
> (cartes ancrées, bascule pastilles ↔ cartes), géométrie splat, édition
> exclusivement dans Data > Annotations.
> Sources amont : `media.md` (D21 — assets référencés, jamais dupliqués),
> `studio.md` (S1–S15 — tours, versioning, viewer pannellum), D2
> (org-tenant), D14 (pagination), D31 (pas de WS navigateur — poll REST).

## 1. Le problème

Les médias (photos, panoramas 360) montrent des installations ; il faut
pouvoir y poser des **annotations** qui affichent du **vivant** : un
device, la valeur d'une pin, l'état (status) d'un device — plus tard des
champs de saisie pour **écrire** des valeurs (control). Exigences
posées :

- **Zéro interférence avec les tours** : le tour studio reste intact,
  les annotations sont un overlay ; un org sans couches publiées voit
  exactement le tour d'aujourd'hui.
- **Versionnable et activable** : le tour et les couches d'annotations
  évoluent avec **leur propre** versioning (cycles de vie indépendants).
- **Compositions** : une annotation posée sur un panorama doit
  apparaître **quand ce panorama est affiché dans un tour**.
- **Édition dans le contexte tour** (mode edit/update), placement par
  clic sur le pano.
- **Multi-salles** : une même salle machine répliquée sur N sites ne
  doit pas multiplier le travail d'annotation.

## 2. Décisions

**D55 — Domaine autonome, ancre = `media_asset_id`.** Une couche
d'annotations (`annotation_layers`) ancre ses items sur des `media_assets`
par UUID — **jamais** sur une scène ni un tour. Le lien tour↔couche reste
**implicite** (médias partagés) : le viewer compose au rendu
(`scene.media_asset_id → items des couches publiées`). Aucune FK, aucune
modification du schéma `tours`/`tour_versions` ; l'ancienne table
`annotations` du domaine `sites` (m000004, droppée sur PG par 000015)
ne gêne pas. C'est le découplage **seul** qui répond au multi-salles :
le tour (la partie studio) est la partie stable, on ne change que les
couches — **pas de duplication de couches** (décision utilisateur), pas
de templates de rôles ; si un org multiplie les salles identiques, il
crée des couches par site et les faits vivre indépendamment (chacune
versionnée, publiable, dépubliable à part).

**D56 — Versioning école S3/S4, activation = publication.**
`annotation_layer_versions` append-only (courant = dernière ; save =
PATCH avec `expected_version_number` → 409 optimiste ; pas de restore —
« restaurer » = charger une vieille version dans l'éditeur puis save).
**Activation = pointeur** `annotation_layers.published_version_id` →
`annotation_layer_versions.id` (FK circulaire PG-only `ON DELETE SET
NULL`, intégrité par le contrôleur sur sqlite — école
`tours.published_version_id` / `flows.deployed_version_id`). Publier une
vieille version = simple repointage ; dépublier = pointeur NULL + les
items disparaissent de tous les viewers. Pas de colonne `is_active`
(aucun précédent dans le repo — publish/deploy sont des pointeurs
purs).

**D57 — Cibles faibles, résolues au read.** Cible d'un item :
`Device{device_id}` | `Pin{device_id, pin_gpio}` | `Status{device_id}` |
`Note{text}` — device identifié par **slug** (`device_registries`),
pin par **gpio** (identité machine stable, pas un label éditable).
`kind` (`device|pin|status|note`) est un string applicatif, cohérence
validée à l'écriture. Résolution **au read, en batch** (école
`link_target_labels` de `services/pois.rs`) : label device, `dead: true`
si le device a disparu — référence morte **tolérée** (école S7 : grisée
dans le popover, jamais 500, jamais panic).

**D58 — Rendu : hotspots pannellum, overlay jamais bloquant.**
Marqueurs panorama = hotspots pannellum `type:"info"` avec
`cssClass "pnex-annot pnex-annot-{kind}"` — classes **strictement
disjointes de `.pnex-hotspot`** (le drag nav résout ses hotspots par
miroir index `_pnexHotspotList` : un marqueur annot qui porterait
`.pnex-hotspot` serait capturé et désalignerait la liste). Reprojection
gratuite par la boucle interne pannellum ; click/drag remontent par
globals JSON seq-guard (école `__pnexTourLastNav`) : `__pnexAnnotClick`,
`__pnexAnnotMove`, `__pnexAnnotPlace` (chaîne JSON **brute**, wasm
`.as_string()`). Ajout/retrait **post-mount** via
`pnexViewers.tour.setAnnotations(hostId, json)` (removeHotSpot/
addHotSpot — **jamais de re-mount**, sinon flicker + rechargement
texture). La popover vit **hors canvas** (panneau dioxus), conteneur
`pointer-events-none` / carte `auto` (précédent bandeau HD de
`MediaPreview` — un overlay bloquant tue drag/zoom, constat device
2026-09-10). Valeurs live en **poll 15 s** (D31, école `pins_panel` :
`GET /devices/{id}/pins` → `PinInfo.last_value` du gpio ; kind status →
`connected`), poll annulé à la fermeture de la popover (compteur de
génération). Images plates (photo/floorplan) : marqueurs absolus en %
du conteneur `relative` de l'`img` (école mini-map de `tour_viewer.rs`).
**Share public exclu V1** : l'endpoint read-model est auth-scoped et
`/share/:token` passe `annotations_enabled: false` (les annotations
peuvent révéler des données device).

**D59 — Édition dans le contexte tour.** Toggle « Éditer les
annotations » (owner|admin, école `can_write`) dans le modal de preview
panorama de l'éditeur studio (et le TourViewer) : modal élargie, layout
flex = viewer + panneau latéral (`AnnotationLayerPanel`). Placement =
**clic sur le pano** (`viewer.mouseEventToCoords(e)` — **synchrone en
pannellum 2.5.7**, retourne `[pitch, yaw]` ; guard < 4 px depuis
mousedown pour ne pas confondre avec un drag caméra, hotspots ignorés)
→ `__pnexAnnotPlace` ; ajustement par **drag du marqueur** (delta px→deg
hfov/w, école `bindTourDrag`) → `__pnexAnnotMove`. Panneau : sélecteur
de couche (couches couvrant le média courant + « créer »), liste d'items
filtrée par média courant, inspecteur (kind, cible via `ResourcePicker`
onglet Device → pins par gpio/label, label, couleur), drawer versions +
publish (école `tour_editor/versions.rs`). Save = validate locale
pnex-core puis PATCH, **modal conflit 409** recharger/écraser (école
tours). V1 : édition **equirect uniquement** (les scènes de tour sont
des panoramas) ; les items `Flat` existent au modèle, leur création se
fera sur la page média plus tard.

**D60 — Portes ouvertes (non construit).** Le modèle laisse la place
sans cassure : (1) **inputs d'écriture** — **révisée par D128
(2026-10-03), livrée 2026-10-04** : un input d'annotation référence un
**contrôle d'org** (`kind: control`), jamais `Pin{…}` ni l'API commands ;
l'effet passe par un flow (§12) ; (2) **géométrie splat** — un variant `Splat` de
`AnnotationGeometry` est additif — **livrée par D147 (§13)** ; (3) **page globale `/annotations`** (listing toutes couches, gestion cross-tours) — **construite 2026-09-16** (Data > Annotations : couches + pickeur média + viewer + panneau, flow sans tour) ; (4) **overlay panorama
de la page média** — **écartée par D147** (la bibliothèque reste le média brut) ; (5) purge des couches par les
labels D42 via un futur `KIND_ANNOTATION_LAYER` au registre.

## 3. Modèle de données (migration `000026`, école 000017)

- **`annotation_layers`** — `id` UUID PK (`uuid_pk`, défaut
  `gen_random_uuid()` PG-only), `org_id` BIGINT NN FK CASCADE, `name`
  varchar(255) NN, `description` TEXT NULL, `published_version_id` UUID
  NULL (FK circulaire **PG-only** SET NULL, ajoutée par ALTER + SQL
  brut), timestamps, `idx_annotation_layers_org`. Pas de colonne
  metadata (droppée des tours par 000018 — le système de labels D42
  couvre).
- **`annotation_layer_versions`** — `id` UUID PK, `layer_id` UUID NN FK
  CASCADE, `version_number` BIGINT NN, `doc` JSONB NN
  (`pnex_core::AnnotationDoc`), `author` varchar(255) NULL, `note` TEXT
  NULL, timestamps ; uniq `(layer_id, version_number)` (SQL brut, **un
  statement par `execute_unprepared`**), idx org.
- Bimoteur : `uuid_pk` PG-only, FK circulaire PG-only (sqlite :
  intégrité portée par le contrôleur, écart documenté en commentaire).

## 4. Doc JSON — `pnex_core::AnnotationDoc`

`crates/pnex-core/src/annotation.rs` — serde-only, ids en **String**
(pnex-core sans dep uuid, wasm32-safe, école `tour.rs`) :

```json
{"items": [{
  "id": "a1", "media_asset_id": "<uuid>",
  "kind": "device|pin|status|note",
  "geometry": {"type": "equirect", "yaw": 45.0, "pitch": -10.0}
           |  {"type": "flat", "x": 0.32, "y": 0.48},
  "color": "#2563eb",
  "label": "PAC-01 · circulateur",
  "target": {"type": "device", "device_id": "pac-01"}
         |  {"type": "pin", "device_id": "pac-01", "pin_gpio": 4}
         |  {"type": "status", "device_id": "pac-01"}
         |  {"type": "note", "text": "Vanne CAU fermée l'été"}
}]}
```

`validate_annotation_doc` (violations FR affichables, école
`validate_tour_doc`) : `duplicate_item_id`, `empty_media_asset_id`,
`invalid_yaw` [-180, 180], `invalid_pitch` [-90, 90],
`invalid_position` [0, 1], `kind_target_mismatch`, `invalid_color`
(`^#[0-9a-fA-F]{6}$` si présent). Asset existe + kind cohérent avec la
géométrie (Equirect → `["panorama"]`, Flat → `["photo", "floorplan"]`)
= validation **backend** au save (batch, école `validate_doc_assets`) ;
devices existants = batch `device_registries`.

## 5. API (additive — CONTRACT jamais bumpé)

Couches, `/api/v1/annotation-layers` (école tours) :

| Endpoint | Notes |
|---|---|
| `GET ""` | D14 : pagination + search name ; hydratation latest + publié |
| `POST ""` | crée couche + v1 (doc vide) → 201 |
| `GET /{id}[?version=n]` | doc (dernière ou demandée) + bindings résolus |
| `PATCH /{id}` | save append-only, 409 optimiste (`expected_version_number`) |
| `DELETE /{id}` | 204, cascade versions (aucun blob — école tours) |
| `GET /{id}/versions[/{n}]` | historique desc / doc historique |
| `POST /{id}/publish` | `{version_number?}` absent = dernière (pointeur) |
| `POST /{id}/unpublish` | pointeur → NULL |

Read model viewers : `GET /api/v1/media/{asset_id}/annotations` — items
**fusionnés des couches publiées** ancrées sur l'asset (union, ordre
déterministe), `layer_id`/`layer_name` par item, résolution device en
batch (`device_pk`, `device_label`, `dead`), scoping org, 404 si asset
inconnu. Deuxième `routes()` dédiée (préfixe `/api/v1/media`) — aucune
collision axum. Droits : écriture owner|admin, lecture membre.

## 6. Rendu — lecture seule

- **TourViewer** (`components/tour_viewer.rs`) : props `annotations_enabled`
  (défaut vrai ; share → false), fetch read-model via `use_resource`
  lisant le memo `scene_asset` **dans la closure** (piège dioxus #2784),
  erreurs avalées → overlay vide (S7) ; toggle « Annotations » (on par
  défaut, rendu si ≥ 1 item) ; application par **effet séparé +
  `set_annotations` post-mount** — la clé de dé-dup du mount reste
  `(scene, url)`, jamais les annotations (sinon re-mount à chaque
  édition). Boucle de poll unique étendue à `take_annot_click` (seq
  monotone) → popover bottom-sheet : label/kind/couche, cible résolue ou
  « cible introuvable » si dead, **valeur live** (poll 15 s, annulé à la
  fermeture).
- **MediaPreview** (`pages/media.rs`) : assets plats → marqueurs en %
  sur l'`img`, même popover (composant partagé
  `components/annotation_editor/popover.rs`).

## 7. Édition — uniquement dans Data > Annotations (D147)

> Historique : l'édition vivait d'abord dans le modal preview du Studio
> (D59), puis a été déplacée (§9 V4), restaurée côté Studio le
> 2026-09-24, et **retirée définitivement par D147** (§13). Le Studio,
> la carte, les médias et les dashboards ne montrent les annotations
> qu'en lecture.

Page `pages/annotations.rs` : liste des ensembles → éditeur. Viewer selon
le support (pano = `TourViewer` sur doc synthétique ; visite = `TourViewer`
sur le VRAI doc, l'ancre suit la scène naviguée ; photo/plan =
`FlatAnnotViewer` ; splat = `SplatHost`) + `AnnotationLayerPanel`
(`w-96`). État = `AnnotationEditorCx` (signaux Copy, école
`TourEditorCx` : `layer_id`, `doc/saved_doc`, `saved_version`,
`selected`, `placing`, `violations` ; `dirty = doc != saved_doc`).
Réducteurs purs dans `state.rs` (testés). Placement/drag = §2 D59 ;
save/publish = §2 D59. Composants `components/annotation_editor/`
(`mod.rs`, `state.rs`, `inspector.rs`, `versions.rs`, `popover.rs`).

## 8. Roadmap par tranches (chacune vérifiable)

1. **pnex-core** : `annotation.rs` + tests (roundtrip serde, bornes,
   kind/target mismatch) → `cargo test -p pnex-core`.
2. **Backend** : migration 000026 + entités + service + contrôleur +
   routes app.rs + `docs/contracts/annotation-layers.http` +
   `tests/annotation_layers.rs` (cycle save 409, publish, read model
   union/dead/drafts exclus, isolation org, rôles) →
   `cargo test -p pnex-backend --test annotation_layers`.
3. **Overlay lecture** : client api (`api/annotation_layers.rs`, école
   `api/tours.rs`, `classify_save_error`) + bridge (`setAnnotations` +
   `take_annot_*`, rebuild `npm run js:viewers`) + TourViewer
   (toggle/popover/live) + MediaPreview plat + i18n (`annot-*` dans les
   DEUX locales) → vérif web manuelle + `task check` natif vert.
4. **Édition tour-contexte** : `annotation_editor/` + intégration
   preview studio + tests réducteurs → vérif manuelle studio.
5. **Portes ouvertes** (D60) — non planifiées.

## 9. Pivot UX (2026-09-16) — la couche n'est plus exposée

Retour utilisateur : « pas besoin de système de layer — une liste
d'annotations par média (photo, pano, ou tour), versionné ; ajouter une
annotation crée l'asset annoté ». Le moteur D55–D60 est conservé
intact (couches, versions append-only, pointeur de publication, read
model union) mais devient un **détail d'implémentation** :

- **V3 (a6f7def) — ensembles nommés avec média associé** : retour
  utilisateur final — une LISTE classique d'objets, chacun avec un nom
  global, un média associé (`media_asset_id`, migration 000027, FK SET
  NULL) et ses annotations ; save refuse tout item hors du média
  déclaré (`AnchorMismatch`, D55 renforcé au niveau couche) ; liste
  filtre `?media=` + chips « autres ensembles sur ce média » pour
  réutiliser plutôt que dupliquer.
- **V4 (000028) — rattachement à un tour, studio débranché** : la
  colonne `tour_id` (FK SET NULL, XOR `media_asset_id` à la création)
  permet d'annoter un TOUR entier : l'éditeur ouvre le tour réel, chaque
  item s'ancre sur le média de la scène naviguée (`on_scene_change` →
  `media_override` → panneau) — D55 reste l'ancre réelle. Validation
  serveur : les items d'un ensemble-tour doivent ancrer sur un média de
  scène du tour **courant** (latest version doc ; si une scène disparaît
  du tour, ses items existants restent mais tout nouvel ancrage y est
  refusé). Filtre `?tour=` + chips par tour. Le chemin studio
  (preview > Edit annotations) est SUPPRIMÉ — tout vit dans Data >
  Annotations ; la preview studio garde l'overlay lecture des couches
  publiées (D58). Le mode implicite `auto:{asset_id}` du panneau est
  retiré (le modal studio était son seul appelant).

## 10. Limitations actuelles (constatées en revue UI, 2026-09-16)

À connaître avant d'utiliser le flow (aucune régression — des portes
D60 restent ouvertes) :

- ~~Pas de déplacement d'annotation~~ **Livré** : drag de
  repositionnement sur les DEUX géométries — pano (delta px → degrés
  hfov/w, pont JS `__pnexAnnotMove` → réducteur `move_item`) et plat
  (pointer events, x/y ∈ [0,1] re-clampés, réducteur `move_item_flat`,
  seuil 4 px distinguant clic-simple/drag, rect re-mesuré en continu).
  E2E validé : plat (0.4,0.6)→(0.75,0.25) persisté après save+reload ;
  pano 0°/0°→8°/-5°.
- **Pas de valeur « métrique »** : impossible d'attacher une valeur
  type ETL ou une mesure device à une annotation. Les cibles actuelles
  (`device`/`pin`/`status`) n'affichent que l'état de connexion et la
  dernière valeur de pin (poll 15 s) — pas de liaison à une métrique
  ETL/thermo, pas d'écriture device (porte D60(1)).

## 11. Pièges verrouillés

- **Classes disjointes** : un marqueur annot ne porte **jamais**
  `.pnex-hotspot` (drag nav = résolution par miroir index).
- **Re-mount churn** : annotations toujours via `set_annotations`
  post-mount ; clé mount = (scene, url) uniquement.
- **Dioxus #2784** : lire les signaux DANS les closures
  `use_resource`/`use_effect` (memo `scene_asset` du fetch annotations).
- **Seq-guard** : 3 nouveaux globals **chaîne JSON brute** (wasm
  `.as_string()`, jamais l'objet parsé — bug historique take_nav
  2026-09-12), seq monotone par global.
- **pointer-events** : conteneur `none`, carte `auto` — jamais d'overlay
  plein écran bloquant (constat device 2026-09-10).
- **i18n** : chaque clé `annot-*` dans les DEUX `.ftl` (test parité) ;
  toute chaîne `{$var}` reçoit ses vars à chaque appel (panic sinon) ;
  sweep `t!` avant commit.
- **Références mortes** : device inconnu → `dead: true` + « cible
  introuvable », jamais panic ; asset inconnu au save → 400.
- **Blob url** : les annotations ne portent pas d'URL ; le switch de
  scène détruit le viewer, `set_annotations` ré-appliqué après chaque
  mount réussi.
- **mouseEventToCoords** : synchrone 2.5.7 mais lit le `config`
  module-global de pannellum — hypothèse **un viewer actif à la fois**
  (modal : OK) ; à re-vérifier si deux viewers coexistent un jour.

## 12. Items `control` et `reading` (D128/D129, 2026-10-04)

Deux kinds additifs, rendus avec les cartes des dashboards
(`components/surface/`, `surfaces-controls.md`) :

- **`control`** : `target = {type: "control", control_id, kind?}`. La carte
  (interrupteur, curseur, bouton, saisie) suit le type du contrôle ; un
  membre l'actionne, un viewer la voit désactivée. L'écriture porte
  `via = annotation:{layer_id}` ; l'effet sur les devices est décrit par
  un flow `control-source` (D127). **D131** : `control_id` nil + `kind` =
  l'item déclare sa propre source, enregistrée au save (origine
  `annotation:{layer}:{item}`) et listée dans le nœud sous cet ensemble ;
  un id d'un contrôle existant = lien (état partagé). Save : contrôle
  inconnu de l'org sans `kind` → 400.
- **`reading`** : `target = {type: "reading", source: SourceRef, spark}` —
  même liaison que les widgets (télémétrie device ou device virtuel de
  flow, mémoire d'org). `spark: true` trace l'historique de la fenêtre
  (widget `line`) ; impossible sur une valeur mémoire (pas d'historique).

Rendu : la popover d'un marqueur affiche la carte ; le viewer de tour
ajoute à droite un panneau « Contrôles et lectures » listant les items
de ce type du média affiché (poll 15 s, D31). Marqueurs
`.pnex-annot-control` (sarcelle) et `.pnex-annot-reading` (bleu ciel).

## 13. D147 — mini-dashboards sur les médias, splat, édition centralisée (2026-10-07)

Retour utilisateur : « on a posé des bases mais c'est pas foufou » — un
ensemble annoté n'était visible nulle part (rien de publié, « Publier »
caché dans le tiroir Historique ; aperçu carte et page Médias montant le
média nu), le viewer splat n'avait jamais fonctionné (API gsplat
inexistante appelée), les contrôles/lectures n'apparaissaient que dans
un panneau latéral, et l'édition restait possible depuis le Studio.

**Décision D147 :**

1. **Édition exclusivement dans Data > Annotations** — pour tous les
   supports : photo, plan, panorama, splat, et visites 360 (ensemble-tour :
   le vrai tour, l'ancre suit la scène). Le mode « Éditer les annotations »
   du Studio est supprimé ; Studio, carte, dashboards = lecture. **La
   bibliothèque (Data > Médias) montre le média brut, sans annotations**
   (retour user 2026-10-07 : « la lib c'est le média cru » — un média peut
   porter des dizaines d'items).
2. **Publication visible** : barre en tête du panneau (statut, « Publier »
   la dernière version enregistrée, alerte tant qu'une version enregistrée
   n'est pas publiée) + bouton **Aperçu** = la vue publiée telle que la
   voient la carte et les visites. Le pointeur de publication
   (D56) reste la seule source des viewers.
3. **Un seul viewer de lecture** : `components/annotated_media.rs`
   (`AnnotatedMediaView`) derrière l'aperçu POI de la carte et l'aperçu
   de la page Annotations (jamais la bibliothèque).
4. **Mini-dashboards sur le média** : les items `control` et `reading`
   deviennent des **cartes** (mêmes corps que les widgets de dashboard,
   `components/surface/`) **ancrées à côté de leur marqueur**. Bascule du
   viewer à 3 états **Masquer / Pastilles / Cartes** (défaut : cartes si le
   média porte des contrôles/lectures, pastilles sinon) ; le panneau
   latéral « Contrôles et lectures » est supprimé (jamais deux polls).
   Positionnement : calque frère du host viewer, cartes `[data-annot-card]`
   placées par la glue (`pnexViewers.cards.follow`) — pano : rect du
   hotspot pannellum ; splat : projection faite par la boucle de rendu ;
   plat : % CSS.
5. **Mini-graphe d'une lecture** : `target.display` ∈ `stat | line |
   gauge | indicator` (+ `min`/`max` pour la jauge), additif ; absent =
   règle historique (`spark` → `line`, sinon `stat`). Une valeur mémoire
   n'a pas d'historique : jamais `line`.
6. **Géométrie splat** (porte D60(2) livrée) : `{type: "splat", x, y, z}`
   en coordonnées monde de la capture, ancrée sur un média `kind=splat`
   (`geometry_media_kinds`). Pose = rayon caméra → centres gaussiens
   (tolérance angulaire ~6 px, alpha ≥ 40, premier groupe de profondeur
   ≥ 3 points — les flotteurs isolés sont ignorés) ; drag = re-pick au
   lâcher. Viewer gsplat 1.2.9 réécrit (WebGLRenderer + OrbitControls,
   cadrage sur le 5–95 % du nuage, clavier désactivé).

7. **Contextes de lecture séparés** (retour user 2026-10-07 : « les
   annotations d'un panorama seul ne doivent pas se mélanger avec celles du
   même panorama dans une visite ») : le read model
   `GET /media/{id}/annotations` ne renvoie que les ensembles **du média**
   (+ ensembles libres sans média ni tour) ; `?tour={id}` ne renvoie que les
   ensembles **de ce tour** (items du média de la scène). Le `TourViewer`
   passe `annotation_tour` (Studio, carte, aperçu Annotations) ; la vue d'un
   média seul n'en passe pas. Avant D147 l'union de toutes les couches de
   l'org touchant l'asset était servie partout.

**Limites assumées :** pas de test d'occlusion sur splat (un marqueur reste
visible à travers la géométrie) ; transform d'objet splat ignoré (les
loaders produisent l'identité) ; formats `.ksplat`/`.spz` sans viewer.

**Pièges :** les cartes ne portent **jamais** `.pnex-annot` (miroir index
du drag pano) ; le calque des cartes est **frère** du host (unmount =
`innerHTML = ''`) ; le worker gsplat **vide** `data.positions` — copie
pour le picking prise juste après le load ; `AnnotationTarget` est
désérialisé à la main (champs numériques sous `arbitrary_precision`,
école `AnnotationGeometry`) ; les événements pano et splat partagent les
compteurs seq (`__pnexAnnot*`, champs `yaw/pitch` ou `x/y/z`).
