# Surfaces pilotables — contrôles d'org, pont surface → flow, format mobile/PC (D123–D129)

> **Statut : PROPOSÉ (2026-10-03)** — à valider avant implémentation.
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

1. Dashboards → **+ Nouveau → Mobile**. Une pile vide s'ouvre, avec une
   section « Général ».
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
