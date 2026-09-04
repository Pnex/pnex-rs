# Moteur de flow ETL « Node-RED full-Rust » — EdgeLinkd vendored, mode B

> **Statut : IMPLÉMENTÉ (Phase 0 + Phase 1, 2026-09-03 — branché
> `worktree-etl-flow-engine` ; Phase 5 — éditeur Dioxus — IMPLÉMENTÉ
> 2026-09-04, branche `flow-engine-phase5-editeur` ; Phase 6 — nœuds
> device/calc/metric + dépendances pin↔flows — IMPLÉMENTÉ 2026-09-04,
> branche `flow-engine-phase6-nodes-device` ; en attente de revue
> humaine).** Ce document consigne (a) la décision d'intégration et de
> rechargement (spike PRD §5.1), (b) les faits **vérifiés** sur EdgeLinkd au
> commit épinglé, (c) les écarts vs la conception initiale.
>
> **Rappel PRD (§3, garde-fous)** : ingestion uniquement (pas de write-side
> device, frontière D13/D17) ; cœur EdgeLinkd jamais patché ; pas de
> type-check global du graphe (contrats typés aux frontières des nœuds
> custom) ; les utilisateurs n'écrivent pas de Rust ; l'éditeur Node-RED
> embarqué n'est **jamais exposé** (runtime headless) ; tranches fines —
> l'éditeur Dioxus est la phase 5 de la piste.

## 0. Phase 5 — Éditeur de flows (2026-09-04)

Éditeur drag & drop complet dans `crates/pnex-frontend/src/components/
flow_editor/`, page `/flows` (liste + éditeur en sous-vue — routes
statiques, pattern `devices.rs`). L'éditeur ne parle **qu'à l'API Loco**
(garde-fou PRD respecté — zéro contact avec le runtime).

Choix structurants :

- **Canevas SVG pur Dioxus** (aucune dépendance npm/Rust ajoutée) — l'SVG
  n'a **pas** de `view_box` : 1 unité utilisateur = 1 px CSS, conversion
  `(client − origine − pan)/zoom`, origine (`getBoundingClientRect`)
  mesurée au début de chaque geste.
- **Gestes** : handlers `move`/`up` sur le root SVG (fiables quand le
  pointeur sort du nœud), `pointerleave` annule un geste orphelin, hit-test
  bbox **mathématique** (pas d'`elementFromPoint`), zoom molette borné
  0.4–2.0 vers le curseur, drag snappé sur grille 20 px.
- **Validation partagée** : `pnex_core::validate_graph` exécutée **dans le
  navigateur** (pnex-core est wasm32) AVANT chaque save — surlignage des
  nœuds en cause + bandeau ; le 400 `{"violations": [...]}` serveur est
  traité à l'identique. Pas de cycle-détection ni de type-check ajoutés
  (garde-fou PRD).
- **Versioning** : save = PATCH avec `expected_version_number` (409 →
  modal « Recharger depuis le serveur / Écraser avec ma version », les deux
  branches repartent d'un détail frais) ; drawer d'historique — « Charger »
  une ancienne version = édition (le prochain save crée v(n+1) avec ce
  graphe), « Déployer cette version » = rollback serveur (ne crée pas de
  version) ; dirty dérivé au rendu (`graph != saved_graph`, jamais de
  signal dirty à tenir à jour — leçon brick0 « zéro set en render »).
- **Inspecteur par kind** : inject/pnex_sql/debug/red, JSON (payload
  inject, config red) validés localement avec drapeau d'invalidité (pattern
  `MetadataEditor` devices) ; les violations du nœud sélectionné sont
  listées dans l'inspecteur (messages pnex-core affichés tels quels).
- **Deploy/rollback** : bouton gated `can_write && !dirty` (deploy =
  publier une version enregistrée) ; chip runtime pollé 5 s
  (`GET /flows/{id}/runtime`) — moteur actif/arrêté, pid/version au
  survol ; 503 `flow_runtime` toasté tel quel.
- **Frontières** : `ApiError` porte désormais `status`/`body` (409/400
  distinguables sans parser le message) ; `Serialize` ajouté sur les DTOs
  requête `pnex-core` (le front construit les requêtes typées) ; handlers
  sans capture de `String` (l'id du nœud ciblé est relu du signal
  sélection — piège FnMut).

## 0bis. Phase 6 — nœuds device/calc/metric (2026-09-04)

La « partie lecture » des devices dans les flows : choisir n'importe quel
device **dans le nœud** (rien d'imposé à la création du flow), lire ses pins,
combiner plusieurs devices, calculer, et écrire le résultat dans OpenObserve
**comme une métrique au même titre que les capteurs**.

Pipeline cible : `[inject] → [device] → [calc] → [metric]`

- **pnex-device** (crate `pnex-node-device`) : dernières valeurs des pins de
  N devices via PromQL `last_over_time(<pin>{device_id="…"}[window])` — la
  **même série** que l'ingestion (`normalize_measurement_name` déplacé dans
  pnex-core pour garantir l'égalité des noms ingestion/lecture). Payload =
  objet `clé → valeur`, clés `sanitize(device) + "_" + sanitize(pin)`
  (`device_payload_key`, prévisualisées à l'identique par l'éditeur) ; lecture
  sans donnée dans la fenêtre = clé omise + warn — **jamais de zéro inventé**,
  le calc aval échoue « variable inconnue » (fail-loud).
- **pnex-calc** : évaluateur d'expressions **maison dans pnex-core**
  (`calc.rs`, pratt parser pur, wasm-safe, zéro dep) — la MÊME fonction
  valide l'expression dans l'éditeur (validation live) et l'exécute au
  runtime. Langage : opérateurs, comparaisons → 1/0, ternaire, `^` droite-
  associatif, 19 fonctions, constantes pi/e ; division par zéro / hors
  domaine = erreur propre, jamais de panic.
- **pnex-metric** : remote-write OpenObserve depuis le runtime — série
  `etl_<nom>` avec `device_id="flow_{id}"` (device **virtuel**),
  `pred_dev="virtual_device"`, `source_type="etl"`, `ts_source="server"`.
  Le préfixe `etl_` est la **séparation d'index** demandée (pas un stream O2
  séparé) : le catalogue Visualisation étant une découverte dynamique des
  streams metrics, la série apparaît d'elle-même « comme un capteur »,
  filtrable par source_type.
- **Creds/org** : `pnex_org_id` estampillé dans l'artefact au deploy
  (`FlowArtifactMeta.org_id` → tab + nœuds custom) — le runtime déduit
  l'org O2 (`pnex_org_{id}`, convention provisioning) **sans accès SQL** ;
  auth Basic racine via allowlist env. Spike exécuté contre l'O2 réel
  (`examples/o2_spike.rs`) : remote-write racine accepté + relecture
  `last_over_time` cohérente — le passcode d'org reste inutile aux lectures
  (O2 v0.92.1), la racine couvre lecture **et** écriture.
- **Évaluateur partagé** (`eval_calc`/`validate_calc`) + nommage centralisé
  (`naming.rs`) + structs prompb en feature (`prompb`) dans pnex-core — le
  front wasm ne compile aucune de ces parties natives.
- **Dépendances pin↔flows** : un `set_mode` in↔out sur un pin scanne les
  flows déployés de l'org dont un nœud device lit ce (device, pin) →
  **dé-déploiement automatique** (status draft, reprojection + SIGUSR1 : le
  flow s'arrête réellement ; la version publiée reste enregistrée), réponse
  enrichie `flow_impacts` → toast UI Pins. Dans l'éditeur, violations de
  **staleness** client-only (pin en sortie, pin disparu, device introuvable)
  → nœud **et câble** en rouge ; re-scan au changement de configuration
  uniquement (pas au drag).

## 1. Architecture cible

```
Workspace PNEX                                    vendor/edgelinkd/ (submodule, épinglé)
├─ crates/pnex-core          modèle typé flow.rs (pur, wasm32)          │
├─ crates/pnex-node-sql      nœud custom (sqlx, SELECT-only) ◀──────────┤ path-dep edgelink-core
├─ crates/pnex-flow-runtime  binaire headless maison ◀──────────────────┘ (features core+js)
├─ crates/pnex-backend (Loco) — NE LIE JAMAIS EdgeLinkd
│   ├─ services/flow.rs             settings.flow (FirmwarePartial)
│   ├─ services/flow_supervisor.rs  process enfant + SIGUSR1 + acquittement
│   ├─ controllers/flows.rs         API /api/v1/flows (versionné, 409, deploy)
│   └─ migration m20260903_000009   flows + flow_versions (append-only)
└─ frontend (Phase 5) : éditeur → API Loco uniquement
```

Flux de déploiement :

```
Éditeur/API Loco → flow_version (Postgres, append-only)
                     │  deploy = publie une version
                     ▼
        Loco projette flows.json (pnex_core::to_red_flows_json,
        TOUS les flows `deployed` de l'instance — le runtime est multi-tabs)
                     │  écriture atomique (tmp+rename)
                     ▼
        SIGUSR1 → pnex-flow-runtime → Engine::redeploy_flows
                     │  acquittement : runtime.json (version projetée ou
                     ▼  compteur de rechargements)
        200 deploy ; artefact porte pnex_flow_id/pnex_version
```

## 2. Spike §5.1 — décision : mode B renforcé (binaire maison)

Le PRD recommandait de démarrer en **process supervisé (B)** et chargeait le
spike de vérifier si une crate moteur réutilisable rendrait A viable plus tôt.
Faits vérifiés au commit épinglé :

- `edgelink-core` est une vraie lib (`Engine::with_json/with_flows_file/start/
  stop/redeploy_flows/subscribe_events`) — consommée en interne par
  `edgelink-pymod` (PyO3) ;
- l'enregistrement des nœuds est **automatique par `inventory`** : tout crate
  lié statiquement s'enregistre via `#[flow_node(...)]` (modèle :
  `node-plugins/edgelink-nodes-dummy`) ;
- `edgelinkd` upstream n'a **ni** hot-reload fichier **ni** SIGHUP
  (uniquement ctrl_c et son API admin web Node-RED, **non authentifiée**) ;
- inject couvre `repeat` + `crontab` (tokio-cron-scheduler) → le besoin
  « cron/interval » de la Phase 4 est déjà couvert.

**Décision : B renforcé.** On ne lance pas le `edgelinkd` upstream : notre
propre binaire `pnex-flow-runtime` lie `edgelink-core` + nœuds PNEX et ajoute
ce qu'upstream n'a pas :

- **rechargement à chaud sans coupure** : SIGUSR1 → relecture du flows.json →
  `Engine::redeploy_flows` (aucune ingestion perdue, aucune surface HTTP) ;
- **stdout = événements JSON-lines machine** (`started`, `debug`,
  `redeployed`, `stopped`…), stderr = logs JSON — le superviseur Loco rejoue
  en `tracing` et déduit la santé de `runtime.json` (pid, flow_rev, redeploys) ;
- **échec = exit(1)** : le superviseur relance avec backoff exponentiel borné.

A reste la cible à terme (EdgeLinkd stabilisé), la porte reste ouverte : le
backend n'a aucune dépendance à la couche transport, seul le superviseur
changerait.

## 3. Contrat de déploiement constaté

| Élément | Valeur (vérifiée au commit épinglé) |
|---|---|
| Artefact | `<state_dir>/flows.json`, tableau Node-RED multi-tabs ; un tab par flow déployé (`id = pnexflow{flow_id}`) |
| Métadonnées | `pnex_flow_id` / `pnex_version` sur le tab et les nœuds custom — préservées par le désérialiseur EdgeLinkd (`#[serde(flatten)] rest`) |
| Rechargement | SIGUSR1 → `Engine::redeploy_flows(json, reg, None)` (stop → re-parse → start) |
| Acquittement | `<state_dir>/runtime.json` : `pid`, `running`, `flow_rev` (SHA-256), `redeploys`, `flow_id`, `version_number` |
| Version incohérente | exit(1) → le superviseur relance avec le fichier courant |
| Secrets | env enfant = `PATH`, `HOME`, `PNEX_FLOW_LOG` + allowlist (`DATABASE_URL`, `OPENOBSERVE_URL`, `OPENOBSERVE_ROOT_EMAIL`, `OPENOBSERVE_ROOT_PASSWORD`) — **jamais** dans flows.json |

## 4. Vérification d'acceptance (Phase 0 + 1)

- (a) `inject → debug` headless lancé/arrêté par Loco —
  `tests/flows.rs::cycle_deploy_edit_rollback_avec_runtime` +
  `pnex-flow-runtime/tests/inject_debug.rs` ;
- (b) flow créé via API (v1 persistée) → déployé → **vraie requête SQL** —
  `pnex-flow-runtime/tests/sql_query.rs` (Postgres requis) ;
- (c) édition → v2 **sans** rechargement (aucun artifact écrit au save),
  deploy v2 rechargé, artefact porte la version 2 ;
- (d) rollback v1 → ancien graphe reprojété ;
- (e) save périmé → **409**, aucune v3 ;
- (f) `msg` malformé rejeté à la frontière du nœud (`pnex_core::SqlQueryRequest`,
  jamais de panic) + validation de graphe en 400 `{"violations": [...]}`.

Empreinte mémoire : la mesure sur Pi (PRD Phase 0) reste un TODO manuel — le
job CI `arm-check` couvre la compilation croisée aarch64/armv7 (`cargo check`
avec cross-compilateurs C : les build scripts de `ring`/`rquickjs-sys`
compilent pour la cible même sans lien). EdgeLinkd revendique ~10× moins de
RAM que Node-RED (non vérifié).

## 5. Écarts vs la conception initiale

- **`features = ["core", "js"]` et non `["core"]` seul** (2026-09-03) : le
  commit épinglé contient un `use rquickjs::...` non conditionné dans
  `variant/mod.rs` — la feature `core` seule ne compile pas. On active la
  feature amont `js` (QuickJS embarqué, ARM OK dans leur CI) — pas de patch
  vendor. À remonter amont ; si corrigé, réduire à `core`.
- **`rust-version` workspace 1.85 → 1.88** : `edgelink-core` (edition 2024)
  utilise les let-chains.
- **Tables physiques plurielées** : le DSL Loco crée `flows`/`flow_versions`
  (`normalize_table` = pluriel cruet) — le PRD parlait de `flow`/`flow_version`
  au niveau conceptuel.
- **FK circulaire PG-only** : `ALTER TABLE ADD CONSTRAINT` n'existe pas en
  sqlite — sur le tier hobbyiste, `flows.deployed_version_id` reste une
  colonne sans contrainte (l'intégrité est portée par le contrôleur) ;
  `schema_invariants.rs` vérifie la contrainte sur PG.
- **409 au lieu de 400** pour les saves périmés : exigence explicite du PRD
  (concurrence optimiste), écart assumé avec la convention 400 historique.
- **Un seul flows.json par instance** : le deploy reprojette l'ensemble des
  flows `deployed` (tous tenants confondus — le runtime exécute multi-tabs).
  L'isolation runtime par org/device est une décision de Phase 3 (attachement
  produit aux devices) — à concevoir avec le modèle de déploiement multi-tenant.
- **Superviseur dans `after_routes`, pas `connect_workers`** : doit vivre aussi
  en ServerOnly (même logique que `spawn_reaper`) ; gate = `settings.flow.enabled`
  uniquement (les tests d'intégration l'activent par env avant boot).
- **Nœuds exclus Phase 1** : `join`, `csv`, `file` partiellement cassés en
  amont (tests de spéc diff) — non utilisés ; à re-évaluer avant la Phase 3.
- **Crédentials Node-RED non implémentés côté EdgeLinkd** : confirmé — notre
  règle « secrets par env seulement » est la seule voie.

## 6. Provenance et règles vendor (`vendor/edgelinkd`)

- Submodule épinglé à `d0a5e114468ee1b26147de55cdca10484ade6b05`
  (master, 2026-01-19 — pas de release versionnée amont : on épingle un SHA).
- **Ne jamais patcher** : extension par nœuds custom + binaire maison
  uniquement ; retours amont = issues/PR ; mise à jour = bump de submodule
  (SHA re-épinglé + note d'écart ici).
- Sous-module amont `3rd-party/node-red` **volontairement non initialisé**
  (~100 Mo, inutile à la compilation) — jamais `git submodule update --recursive`.
- Licence Apache-2.0 (code séparé, non modifié) — compatible AGPL-3.0-or-later
  du workspace.
