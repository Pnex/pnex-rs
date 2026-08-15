# PNEX — Migration Django → Rust (Loco + Dioxus)

> Document de pilotage pour Claude Code. À suivre **phase par phase**, dans
> l'ordre. Ne pas passer à la phase N+1 tant que le « Done when » de la phase N
> n'est pas vert. S'arrêter pour revue humaine à chaque frontière de phase.

## 1. Contexte et source de vérité

- Le **projet Django existant fonctionne** : il est la **spécification exécutable**.
  Chaque comportement porté doit être tracé à son fichier Django d'origine.
- Deux codebases sources à replier dans le nouveau workspace :
  - Backend : **Django 6 + DRF + Channels** (WebSocket).
  - Web UI : codebase **séparée** (à réintégrer).
- On ne fait **pas un port ligne à ligne** : c'est une réécriture guidée par le
  contrat d'API et le comportement observable du Django.

## 2. Architecture cible

Workspace Cargo unique :

```
pnex/
├── Cargo.toml            # [workspace]
├── Taskfile.yml          # orchestre cargo loco + dx
├── crates/
│   ├── core/             # types/DTO serde partagés — compile wasm32 ET natif
│   │                     #   (aucune dép native : pas de tokio/sqlx ici)
│   ├── backend/          # appli Loco : API + models SeaORM + worker de build
│   │                     #   + câblage OpenObserve + sert les assets WASM
│   ├── frontend/         # appli Dioxus, feature `web` (CSR pur), appelle l'API
│   │                     #   Loco via gloo-net
│   └── firmware-builder/ # lib de build appelée par le worker Loco
```

Décisions structurantes (déjà arrêtées, à respecter) :

- **Serveur** : **Loco possède le serveur Axum et l'API.** Dioxus est un front
  **web/CSR pur** (feature `web`, **pas** `fullstack`/`ssr`) → pas d'hydratation
  SSR attendue. On **n'utilise pas** les server functions de Dioxus (elles
  exigeraient le serveur Axum de Dioxus → collision avec Loco). Le front appelle
  l'API Loco explicitement, types partagés via `core`.
- **Boucle de régulation** : **hors périmètre du serveur.** Le central est un
  point de **collecte + ETL + broadcast de config** (archi SCADA/historian),
  **sans logique d'action**. La commande vit à l'edge (M2M).
- **Thermo** : CoolProp reste un **service FastAPI externe** (Python). Ne pas le
  réécrire en Rust.
- **Formules utilisateur** : sandbox d'évaluation en **WASM via wasmtime**
  embarqué dans le cœur Rust ; thermo déléguée au service CoolProp.
- **Déploiement** : reste **Kubernetes via ArgoCD/GitOps**. Secrets **SOPS/age**.

## 3. Simplifications d'architecture (avant → après)

Table maîtresse de la migration. **Tout ce qui est marqué « supprimé » ne doit
pas être réintroduit**, sous aucune forme.

| Avant (Django / actuel)              | Après (cible)                                              | Pourquoi |
|--------------------------------------|-----------------------------------------------------------|----------|
| **CockroachDB** (SQL distribué)      | **PostgreSQL**                                            | Pas de besoin de SQL distribué ; PG couvre relationnel + config. |
| **Elasticsearch** (index par user)   | **OpenObserve** (+ PostgreSQL)                            | OpenObserve couvre recherche/logs/télémétrie ; testé standalone + cluster. |
| **NATS** (pub/sub)                   | **supprimé** — WS (ingestion) + broadcast desired-state (config) + pipelines OpenObserve ; eventing interne éventuel via **Postgres LISTEN/NOTIFY** | Une brique de messaging en moins. |
| **Redis + Celery**                   | **supprimé** — worker **Loco** sur **queue Postgres**     | Jobs durables sans Redis ; « que du PG ». |
| **Argo Workflows**                   | **supprimé** — le build est un **job Loco** (queue PG)    | Plus d'orchestration de pods à la volée. |
| **Pod K8s par actuateur**            | **supprimé** — régulation à l'**edge** (M2M)              | Latence mesure/compute/commande trop longue via le central. |
| **Pod K8s par build firmware**       | **supprimé** — **job Loco** qui shell-out le toolchain    | Complexité Kube inutile ; toolchain chaud = builds plus rapides. |
| **Django + DRF + Channels** (3)      | **Loco** (1 framework, sur Axum)                          | Un seul framework backend. |
| **Web UI** (repo séparé)             | crate **Dioxus** dans le **même workspace**              | Fin de l'éparpillement ; une seule repo. |
| Front JS ↔ contrat DRF (à maintenir) | types **`core`** partagés front/back                     | Source de vérité unique, zéro dérive DTO. |
| **Logique de contrôle** côté serveur | déplacée à l'**edge** (SCADA/historian)                   | Le central ne fait plus que données + config. |
| **K8s dynamique** (pods à la volée)  | déploiement **K8s statique** via ArgoCD                   | Manifests simples, plus de gestion de pods runtime. |
| **Python** (backend)                 | **Rust** (sauf service CoolProp)                          | Un seul langage côté plateforme. |

### Conservé (ne pas supprimer)

- **PostgreSQL** (c'est déjà la DB Django) — devient le pivot relationnel/config.
- **Keycloak / OAuth2** — auth et multi-tenant.
- **Chiffrement WS ChaCha20-Poly1305** — canal d'ingestion.
- **Service CoolProp** (FastAPI externe) — thermo.
- **ArgoCD / GitOps + SOPS/age** — déploiement et secrets.
- **MinIO / stockage objet** — *à confirmer* : binaires firmware + assets WASM.
  Décider explicitement s'il est conservé ou remplacé (disque / PG large objects)
  avant la Phase 6. Ne rien supposer.

### Effet net

- Stockage/messaging : ~5 briques → **2** (PostgreSQL + OpenObserve) [+ stockage
  objet à confirmer].
- Frameworks backend : **3 → 1** (Django/DRF/Channels → Loco).
- Repos : **2 → 1** workspace.
- Orchestration : K8s **dynamique → statique** (plus d'Argo Workflows ni de pods
  à la volée).
- Rôle du serveur : contrôle temps réel **+** données → **données seules**
  (collecte / ETL / config).

## 4. Principes de migration

- **Tranches verticales** : chaque fonctionnalité va de bout en bout —
  type `core` → migration+model Loco → endpoint Loco → vue Dioxus → test de
  parité vs Django — avant de passer à la suivante.
- **Le workspace compile en permanence.** À chaque étape : `cargo check` vert,
  `dx build --platform web` vert, tests verts. Jamais de commit qui casse le build.
- **Parité prouvée** : capturer les contrats DRF (endpoints, payloads d'exemple)
  et écrire des tests qui asservissent le comportement Rust à ces contrats.
- **Minimal et ciblé** : pas de sur-ingénierie, pas d'abstraction spéculative.
- **Tracer l'origine** : pour chaque comportement porté, citer en commentaire le
  fichier/fonction Django source.

## 5. Hors périmètre — NE PAS porter

Voir la table §3. En résumé : le modèle **pod-par-actuateur**, **Argo Workflows**,
les spécificités **CockroachDB / Elasticsearch / NATS / Redis-Celery**, la
réécriture de **CoolProp**, et toute **logique d'action/contrôle temps réel**
côté serveur.

## 6. Phases

### Phase 0 — Inventaire & capture des contrats
- **But** : comprendre l'existant, produire la carte de migration. **Aucun code
  Rust.**
- **Étapes** : explorer les deux repos Django ; lister modèles, endpoints DRF,
  consumers Channels, tâches Celery, dépendances externes ; produire
  `docs/inventory.md` (table : élément Django → cible Rust → phase, en marquant ce
  qui est **supprimé** par la §3) ; capturer des exemples de requêtes/réponses DRF
  dans `docs/contracts/`.
- **Done when** : `inventory.md` + contrats commités et revus par l'humain.

### Phase 1 — Squelette du workspace
- **But** : un workspace qui compile, vide de logique métier.
- **Étapes** : créer les 4 crates ; `core` avec un type bidon compilant en
  wasm32+natif ; app Loco minimale (health route) ; app Dioxus `web`/CSR minimale
  servie en statique par Loco ; `Taskfile.yml` (`task dev`, `task build`) ; CI
  (`cargo check`, `dx build --platform web`, `cargo test`) ; découpage des
  features `web`/`ssr`/`server` documenté pour éviter le piège d'hydratation.
- **Done when** : `task build` vert, CI verte, `task dev` sert la page vide.

### Phase 2 — Couche données
- **But** : schéma PG + migration des données.
- **Étapes** : traduire les modèles Django en migrations SeaORM ; script de
  migration Cockroach→PG (wire-compatible) ; brancher OpenObserve pour la
  télémétrie/logs (ex-Elastic) ; vérifier l'intégrité (comptes de lignes,
  échantillons).
- **Done when** : schéma appliqué, données migrées, checks d'intégrité verts.

### Phase 3 — Auth & multi-tenant
- **But** : parité d'authentification et d'isolation tenant.
- **Étapes** : intégration OAuth2/Keycloak dans Loco ; middleware de scoping
  tenant ; tests : un tenant ne voit jamais les données d'un autre.
- **Done when** : tests d'auth et d'isolation verts contre les contrats Django.

### Phase 4 — Gestion des devices (CRUD)
- **But** : première tranche verticale complète, patron pour les suivantes.
- **Étapes** : types `core` ; endpoints Loco ; vues Dioxus (liste/détail/édition)
  via gloo-net ; tests de parité vs DRF.
- **Done when** : CRUD device de bout en bout, parité prouvée.

### Phase 5 — Ingestion télémétrie + ETL + broadcast config
- **But** : le rôle SCADA/historian du central.
- **Étapes** : endpoint WS (ChaCha20-Poly1305) pour l'ingestion ; pipelines/
  fonctions ETL côté OpenObserve ; **broadcast de config en desired-state** (le
  central publie « version N + hash », l'edge pull le delta) ; **aucune logique
  d'action** ici.
- **Done when** : télémétrie ingérée et visible en dashboard ; config poussée et
  réconciliée par un edge de test.

### Phase 6 — Worker de build firmware
- **But** : compilation firmware sans pod, dans Loco.
- **Étapes** : trancher le sort du stockage objet (§3 Conservé) ; job Loco sur
  queue Postgres ; `firmware-builder` shell-out du toolchain en child process ;
  workspace tmp par job ; **injection de secrets scopée + effacement** ; timeout
  dur + kill ; cache proxy de dépendances ; plafond de concurrence.
- **Done when** : un build produit un binaire fonctionnellement identique à la
  chaîne Django, secrets non fuités entre jobs, timeout testé.

### Phase 7 — Flash navigateur (WebSerial)
- **But** : flash ESP32 depuis le navigateur, en Dioxus.
- **Étapes** : composant Dioxus appelant l'API Web Serial via web-sys/wasm-bindgen ;
  parcours upload firmware → flash → feedback.
- **Done when** : flash d'un ESP réel réussi depuis l'UI.

### Phase 8 — Moteur de formules
- **But** : évaluation des formules utilisateur + thermo.
- **Étapes** : client backend vers le **service CoolProp FastAPI** ; **sandbox
  wasmtime** pour les formules/fonctions utilisateur ; tests de parité numérique
  vs Django sur un jeu de formules de référence.
- **Done when** : résultats identiques (à tolérance près) au moteur Django.

### Phase 9 — Bascule & décommissionnement
- **But** : couper Django, nettoyer.
- **Étapes** : bascule module par module ; **retrait Django / Argo Workflows /
  CockroachDB / Elasticsearch / NATS / Redis-Celery** ; manifests ArgoCD mis à
  jour (SOPS/age) ; runbook DR ajusté.
- **Done when** : plateforme servie 100 % par Loco+Dioxus, anciennes briques de la
  §3 retirées, GitOps à jour.

## 7. Méthode de travail (Claude Code)

- **Une branche par phase** ; petits commits atomiques traçant la source Django.
- **À chaque étape** : `cargo check`, `dx build --platform web`, `cargo test`.
  Ne jamais commiter rouge.
- Tenir un **`PROGRESS.md`** : phase courante, fait, décisions, points ouverts.
- **S'arrêter pour revue** à chaque frontière de phase.
- **Demander avant** toute opération destructive (drop de données, suppression de
  code Django, migration irréversible).

## 8. Garde-fous

- Ne **jamais réintroduire** un composant marqué « supprimé » en §3.
- Ne **jamais inventer** un endpoint ou un champ : si le contrat Django est
  ambigu, s'arrêter et demander.
- **Secrets** uniquement via SOPS/age ; jamais en clair dans le repo ni dans un
  log de build.
- Préférer la **modification minimale** à la refonte.
- `core` doit rester **compilable en wasm32** : refuser toute dép native qui s'y
  glisse.
