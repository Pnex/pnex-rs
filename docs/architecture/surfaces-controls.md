# Surfaces pilotables — contrôles d'org, pont surface → flow, format mobile/PC (D123–D129, D131–D133)

> **Statut : LIVRÉ (L1–L7, 2026-10-04)** — décisions validées le 2026-10-03,
> implémentation et écarts consignés au §4. **Révision du 2026-10-04
> (D131–D133, §5)** : la surface déclare, le moteur enregistre — plus de
> contrôle à choisir avant de pouvoir enregistrer un dashboard.
> Docs liés : `viz-bases.md` (D24 versioning, D31 polling, D40 canvas libre,
> D41 bibliothèque), `annotations.md` (D55–D60, porte ouverte « inputs »),
> `flow-engine.md` (nœuds custom, deploy, fencing D106), `inventory.md`
> (D13, D17 amendé : une seule source d'écriture par pin).

## Le concept global

Le principe tient en une phrase : **une surface lit, un flow agit.**

- Une **surface** est tout ce que l'on regarde : un dashboard PC, un
  dashboard mobile, une annotation sur un panorama 360 ou une photo, et
  plus tard le tiroir d'un POI.
- **Lecture directe**, sans passer par un flow. Une surface affiche :
  - une métrique d'un device (télémétrie) ;
  - une clé de mémoire d'org en Valkey (alimentée en direct par un flow) ;
  - une mini-courbe (sparkline) tirée d'O2.
- **Action indirecte, toujours par un flow.** Une surface ne commande
  jamais un pin. Elle écrit la valeur d'un **contrôle**, et un flow
  abonné à ce contrôle décide de l'effet sur 1 ou N devices.

Ce schéma impose le flow comme seul endroit où le comportement est décrit.
Il y est visible, versionné, déployé par l'utilisateur et débogable : un
clic sur un switch, c'est un message que l'on voit passer dans le debug du
flow. C'est l'inverse des `on_turn_on: lambda` dispersés dans le YAML
ESPHome.

Les contrôles sont des **entités d'org**, pas des propriétés d'un
dashboard. Un même contrôle « Éclairage salle machine » peut donc
apparaître sur le dashboard mobile, sur l'annotation du panorama de la
salle et sur le dashboard PC, avec le même état et le même flow derrière.
C'est le pendant symétrique de la mémoire d'org :

| | Écrit par | Lu par | Clé Valkey |
|---|---|---|---|
| **Mémoire** (existe) | flow (`memory-write`) | surfaces, flows (`memory-read`) | `pnex:mem:v1:{org}:{key}` |
| **Contrôle** (nouveau) | surfaces (humain) | flows (`control-source`), surfaces | `pnex:ctl:v1:{org}:{control_id}` |

## 0. Décisions

| # | Décision | Motif |
|---|---|---|
| D123 | **Le format d'un dashboard est choisi à la création et ne change plus.** `DashboardLayout.format` vaut `desktop` (canvas libre D40, comportement actuel) ou `mobile` (D124). Les dashboards existants désérialisent en `desktop` (`serde(default)`). « + Nouveau » propose PC ou Mobile, sans modale, puis crée le dashboard directement. Pour changer de format, on duplique. | Choix utilisateur (2026-10-03) : un seul format par dashboard, pour ne pas placer chaque widget deux fois. |
| D124 | **Format mobile = composer de cartes, sans positionnement en px.** `layout.sections = [{id, title, items: [widget_id]}]` ; cartes sur 2 colonnes (`span` 1 = demi-largeur, 2 = pleine largeur) ; ajout depuis la palette, réorganisation par glisser entre sections. `x/y/w/h` sont ignorés. Les `wires` sont interdits (`mobile_wires_forbidden`). Chaque widget appartient à exactement une section. | Choix utilisateur : un composer « à la ESPHome », c'est-à-dire une page lisible au pouce. |
| D125 | **Les contrôles sont des entités d'org.** Table `controls` (UUID, `org_id`, `key` unique par org au motif `[A-Za-z0-9_.-]{1,64}`, `label`, `kind`, `spec` JSON). Pas de versioning : école D41, état mutable simple. Types : `switch` (`on`/`off`, 1/0 par défaut), `slider` (`min/max/step`, 0/100/1 par défaut, donc directement un duty PWM), `button` (impulsion, valeur fixe), `number` (saisie bornée). `ControlSpec::accepts(value)` est partagé entre le front (wasm) et le serveur. **Un contrôle n'est jamais lié à un pin.** Gestion depuis une page Data > Contrôles et création à la volée depuis n'importe quelle surface. La suppression d'un contrôle utilisé par un flow déployé est refusée (409 `control-in-use`). Un contrôle qu'**aucun flow déployé** n'écoute s'affiche avec le badge « sans effet », pour qu'on ne croie pas avoir actionné quelque chose. | Un contrôle porte une intention réutilisable sur toutes les surfaces. L'attacher à un dashboard aurait obligé à le recréer pour les annotations. |
| D126 | **Écrire un contrôle, c'est stocker et publier une valeur, jamais envoyer une commande device.** `POST /api/v1/controls/{id}/value {value}` : validation par `accepts`, puis pipe Valkey `SET pnex:ctl:v1:{org}:{id} {v, ts_ms, by, via}` sans TTL (la dernière valeur commandée persiste) et `PUBLISH pnex:ctl:v1:{org}`. `via` = la surface d'origine (`dashboard:{id}`, `annotation:{id}`), informatif. La lecture des valeurs se fait par le polling des surfaces (D31), avec un rafraîchissement immédiat après chaque écriture. Droits : un membre écrit ; un viewer voit les contrôles désactivés ; un partage public est toujours en lecture seule. Chaque écriture est journalisée dans le flux d'événements O2 (D86) : qui, quand, quelle valeur, depuis où. Débit limité par contrôle : le slider envoie au relâchement, avec un plafond serveur d'environ 4 écritures par seconde (`control-rate-limited`). | Même école que la mémoire d'org et que camera-source (pub/sub Valkey). Le serveur ne touche jamais un device. |
| D127 | **Nouveau nœud `pnex-control-source`** (kind `ControlSource`), de config `{controls: [id], emit_on_start}`. Source **événementielle** : `SUBSCRIBE pnex:ctl:v1:{org}` et filtre sur ses contrôles ; la reconnexion avec backoff et le heartbeat de statut reprennent camera-source. Il a un port de sortie par contrôle (routage par topic, école json-split/memory-read). Chaque msg porte `payload` = la valeur, `topic` = la `key` du contrôle et `msg.control = {id, key, by, via, ts_ms}`. `emit_on_start` lit les valeurs courantes (MGET) au démarrage ou au redeploy et les réémet, ce qui fait retrouver l'état des sorties après un redémarrage. Le deploy vérifie que les contrôles existent dans l'org. Un contrôle supprimé hors deploy donne un statut de nœud « contrôle absent », jamais un crash. Un switch (1/0) ou un slider (0..100) se branche tel quel sur `device-write` (DigitalOut 0/1, PWM 0..100), et un même contrôle peut alimenter N `device-write`. | C'est le chaînon demandé (« source dashboard → valeur ») : ensuite, toute la mécanique de flow classique s'applique. Le nœud référence le contrôle, pas la surface : déplacer un switch d'un dashboard vers une annotation ne casse aucun flow. |
| D128 | **Frontière D13/D17 : les surfaces sont des entrées de flows, jamais un chemin d'actionnement.** Le non-but de `viz-bases.md` (« la visualisation n'est jamais un chemin de contrôle ») **reste vrai** au sens strict : la seule voie vers un pin est un flow déployé par l'utilisateur avec `device-write`. La règle « une source d'écriture par pin » (8d60950), la porte `pin-already-assigned` et `reserved_by` sont inchangées. **D60 (1) révisée** : un input d'annotation référence un **contrôle**, jamais `Pin{device_id, pin_gpio}` ni l'API commands. L'actionnement manuel depuis la page Pins (D17) reste l'outil de prototypage, hors surfaces. | Un write direct depuis une surface serait une seconde source d'écriture concurrente du flow et cacherait le comportement hors de tout flow. |
| D129 | **Les liaisons de lecture sont communes à toutes les surfaces.** `SourceRef` (`viz.rs`) est la liaison unique de lecture : télémétrie device, mémoire d'org, et un nouveau mode **sparkline O2** (fenêtre 5m..24h via `series-batch`, mini-courbe sans axes). Un **binding de contrôle** `ControlRef {control_id}` s'y ajoute, avec une `source` d'état facultative : l'état réel remonté par le device, qui, s'il est absent, retombe sur la dernière valeur commandée. Les widgets de dashboard et les items d'annotation (nouveau kind `control` et kind `reading` + sparkline) partagent ces types et un même composant de rendu (`components/surface/`). | Une seule implémentation pour les cartes mobiles, les widgets PC et les panneaux d'annotation ; les surfaces futures (tiroir POI) ne réécrivent rien. |

## 1. Parcours cible (« tuer le YAML ESPHome »)

> Révisé par D131–D133 (§5) : l'étape 2 n'exige plus de contrôle ; tout
> interrupteur posé devient une source référencée, et le flow se construit
> avant ou après le dashboard, au choix.

1. Dashboards → **+ Nouveau** : une modale présente PC et Mobile (aperçu,
   usages) ; on nomme, on choisit **Mobile**. Une pile vide s'ouvre, avec
   une section « Général ».
2. **Ajout guidé depuis un device** : on choisit un device, et le composer
   liste ses pins et ses métriques avec un widget proposé pour chacun :
   - sortie digitale → nouveau contrôle `switch` ;
   - PWM → `slider` ;
   - capteur → jauge, stat ou sparkline.
3. Sur un contrôle, **« Créer le flow »** génère un brouillon
   `control-source(contrôle)` → `device-write(device, pin)` et l'ouvre dans
   l'éditeur de flow. On peut y ajouter de la logique : conditions,
   plusieurs devices, horaires. Ensuite on déploie.
4. Le même contrôle se pose sur l'annotation du panorama de la pièce :
   mêmes état et flow.
5. Usage : on bascule le switch, le debug du flow montre le msg, la sortie
   s'allume, et la source d'état du widget affiche la valeur réelle.

## 2. Lots

| Lot | Contenu | Garde |
|---|---|---|
| L1 core | `pnex_core::control` (`ControlKind`, `ControlSpec::accepts`, `ControlRef`), `format` / `sections` / `span`, widgets `switch`/`slider`/`button`/`number`/`sparkline`, `validate_layout` étendu | tests unitaires `pnex-core` |
| L2 backend | migration `controls` (PG + SQLite, parité D120), CRUD `/api/v1/controls`, `POST …/value`, `POST /controls/values` (lecture groupée), Valkey SET + PUBLISH, droits, débit, événement O2, codes d'erreur + clés ftl | tests contrôleur + `error_codes.rs` |
| L3 nœud | `pnex-control-source` (8 points de câblage, abonnement sur le modèle de camera-source), validation au deploy, 409 `control-in-use`, outil IA `describe_node_types` | e2e runtime : POST value → msg debug |
| L4 front dashboards | choix PC/Mobile, éditeur composer mobile + rendu live, `components/surface/` (contrôles + lecture + sparkline), page Data > Contrôles | `i18n_guard`, build wasm |
| L5 composer guidé | ajout depuis device/pin, « Créer le flow » (brouillon pré-câblé) | e2e Playwright : switch → LED réelle |
| L6 annotations | items `control` / `reading` sur pano et photo via `components/surface/`, D60 mise à jour | e2e annotation → flow |
| L7 docs | `viz-bases.md` (non-but précisé, schéma JSON à jour), `annotations.md`, `flow-engine.md`, `inventory.md` §0, roadmap | — |

## 3. Questions ouvertes

- **Rôle « opérateur »** (opère sans éditer) : hors scope. Pour l'instant,
  seuls les membres écrivent.
- **`select`** (liste de valeurs) : à ajouter quand un cas réel le
  demandera.
- **Droits par contrôle** (restreindre un contrôle à certains membres) :
  hors scope ; à reprendre avec le rôle opérateur.

## 4. Implémentation (2026-10-03 → 2026-10-04)

| Lot | Livré | Où |
|---|---|---|
| L1 | Modèle partagé navigateur/serveur : `ControlKind`, `ControlSpec::{check, accepts}`, `ControlRef`, `ControlSourceConfig` ; `DashboardLayout.format` / `sections`, `WidgetOptions.control` / `span` ; `validate_layout` (règles mobile, widgets de contrôle) | `pnex_core::ui_control`, `pnex_core::viz` |
| L2 | Table `controls` (PG + SQLite, parité D120), `/api/v1/controls` (CRUD, `…/value`, `values`), Valkey `SET` + `PUBLISH` avec débit `SET NX PX 250`, journal O2 `ev_controls`, 409 `control-in-use`, porte de deploy `control-unknown` | `services/controls.rs`, `controllers/controls.rs` |
| L3 | Nœud `pnex-control-source` (crate `pnex-node-ui-control`) : SUBSCRIBE, rejeu MGET après abonnement, statuts `control-listening` / `control-bus-unavailable` ; palette Déclencheurs, inspecteur avec création à la volée | `crates/pnex-node-ui-control`, `flow_editor` |
| L4 | « + Nouveau » → tuiles PC / Mobile ; badge de format en liste ; `format_immutable` au save ; composer mobile (sections, cartes ½ / pleine largeur, glisser-déposer + flèches) ; cartes de contrôle live (`components/surface/`) ; page Data › Contrôles | `dashboard_editor/mobile.rs`, `components/surface/`, `pages/controls.rs` |
| L5 | Palette « Depuis un device » : sorties digitales → interrupteur, PWM → curseur (contrôle créé, ou réutilisé si la clé existe), mesures → valeur / jauge / courbe ; « Créer le flow » (brouillon `control-source` → `device-write`, ouvert dans l'éditeur, jamais déployé automatiquement) | `dashboard_editor/device_panel.rs`, `control_panel.rs` |
| L6 | Annotations `control` / `reading` (cf. `annotations.md` §12) | `pnex_core::annotation`, `components/surface/annotation.rs` |
| L7 | Cette section, `viz-bases.md`, `annotations.md`, `flow-engine.md`, `inventory.md`, `roadmap.md` | — |

Écarts et précisions par rapport aux décisions :

- **Sparkline** : pas de nouveau type de widget, c'est le widget `line`
  existant (fenêtre `5m..24h` via `series-batch`). Un item d'annotation
  `reading` porte `spark: true` pour l'afficher.
- **`via`** : `dashboard:{id}` pour un dashboard, `annotation:{layer_id}`
  pour une annotation (la couche, pas l'item).
- **Source d'état** d'une carte de contrôle : `source[0]` facultative, de rôle
  `state`. La carte affiche « État réel : … » sous la valeur commandée.
- **Contrôle supprimé** : la carte affiche « Contrôle supprimé ». Le nœud
  n'a pas de statut dédié : la suppression est refusée tant qu'un flow
  déployé l'écoute, et le deploy refuse un contrôle inconnu.
- **Annotation** : le save refuse un contrôle inconnu de l'org (comme un
  device inconnu) ; une lecture peut viser un device virtuel de flow
  (`flow_{id}`), jamais enregistré, donc non vérifié.
- **Débit** : deux actions sur le même contrôle à moins de 250 ms → 429
  `control-rate-limited` ; le curseur n'écrit qu'au relâchement.

## 5. Révision « la surface déclare, le moteur enregistre » (D131–D133, 2026-10-04)

Retour utilisateur après la livraison : poser un interrupteur exigeait de
choisir un « contrôle piloté » existant, sans quoi l'enregistrement était
refusé (`control_missing`). L'utilisateur devait donc créer le contrôle,
voire le flow, avant le dashboard : un interblocage. Le nœud `control-source`
sans contrôle coché n'avait pas de sortie visible, et les contrôles n'y
étaient pas rattachés à leur surface.

| # | Décision | Motif |
|---|---|---|
| D131 | **Un widget de contrôle (ou un item d'annotation `control`) déclare sa propre source ; le serveur l'enregistre à la sauvegarde de la surface.** Dans la transaction du save, chaque item sans contrôle reçoit un contrôle d'org dont `origin` = `{dashboard|annotation}:{uuid surface}:{id item}` (colonne `controls.origin`, migration 000003, unique par org). Clé générée stable `dash-<8 hex>.w-0001` / `annot-<8 hex>.<item>` (suffixe `-2`… en cas de collision), libellé = titre de l'item (sinon son id), qui suit le titre aux saves suivants. Le document stocké et renvoyé porte les ids liés ; l'éditeur les adopte. Un item d'annotation déclare le **type** de sa commande (`target.kind`, `control_id` nil jusqu'au save). **Amende D125** : la création à la volée devient implicite ; lier un contrôle existant (état partagé entre surfaces) reste possible en option avancée. Un id de contrôle inconnu de l'org (supprimé, ou d'une autre org) n'est jamais lié : l'item reçoit sa propre source (dashboard), ou le save est refusé s'il ne déclare pas de type (annotation). Retirer l'item ou supprimer la surface **libère** sa source : supprimée si plus rien ne la référence (dernière version et version déployée des flows, version courante des autres dashboards, dernière et publiée des autres ensembles d'annotations), sinon conservée comme contrôle indépendant (`origin` vidée) pour qu'aucun flow ne perde son déclencheur. | L'utilisateur construit la surface d'abord ou le flow d'abord, sans passage obligé ; chaque élément posé est référençable (« Tableau › #w-0001 ») et réutilisable dans les flows. |
| D132 | **« + Nouveau tableau de bord » ouvre une modale** : nom, deux cartes de format (aperçu dessiné, résumé, trois usages types), rappel que les commandes posées deviennent des sources pour les flows, et que le format est définitif. **Amende D123** (« sans modale ») ; le format reste fixé à la création. | Les tuiles en ligne n'expliquaient pas la différence entre les formats ni leur usage. |
| D133 | **Le nœud `control-source` est un catalogue des sources** : groupées par surface (« Tableau de bord · Salle machine », « Annotations · Pano RDC », puis « Contrôles indépendants »), référence `#w-0001 · libellé`, recherche au-delà de 6 sources. Il a **toujours** sa sortie à droite (un port non libellé tant que rien n'est coché) et **aucune entrée** (un câble ne peut pas y aboutir). Un nœud sans source s'enregistre comme brouillon (le flow peut précéder ses surfaces) ; seul le deploy l'exige (`control_source_empty`, porte de deploy et construction du runtime). La page Contrôles affiche la colonne « Déclaré par ». **Précise D127** (config inchangée : `{controls: [id], emit_on_start}`). | Le nœud apparaissait comme une entrée (point à gauche) sans sortie ; les sources doivent se retrouver par leur surface. |

Implémentation : `pnex_core::ui_control` (`control_origin`, `auto_control_key`,
`ControlOrigin`, `UiControl.origin`), `validate_layout` sans
`control_missing`, `AnnotationTarget::Control.kind` ;
`services/surface_controls.rs` (`sync_surface`, `release_surface`,
`resolve_origins`) appelé par `services/dashboards.rs` et
`services/annotation_layer.rs` dans leur transaction ; front :
`dashboard_editor/control_panel.rs`, `annotation_editor/surface_targets.rs`,
`flow_editor/inspector/control_source.rs`, `pages/dashboards.rs`
(modale). Gardes : `tests/controls.rs`
(`surface_declared_controls_are_provisioned_and_released`),
`tests/annotation_layers.rs`, e2e `controls.spec.ts` (le switch posé sans
contrôle pilote un flow déployé).

Limite connue : restaurer une ancienne version d'un dashboard (déplacement
de pointeur, D24) ne re-provisionne rien ; une carte dont la source a été
libérée entre-temps affiche « Contrôle supprimé » et retrouve une source au
prochain enregistrement.
