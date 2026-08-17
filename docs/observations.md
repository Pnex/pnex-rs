# Observations de terrain — registre des constats à traiter

> Constats faits en conditions réelles (UI navigateur, e2e toolchain réelle,
> base dev). Chaque entrée : symptôme → cause racine → correction proposée →
> statut. Les entrées corrigées restent pour la traçabilité. À réviser à la
> planification de chaque phase.

## 2026-08-16 — session UI après la Phase 6 (branche `phase-6-firmware-builder`)

### O1 — Fallback d'org après re-login : atterrissage sur une org viewer

- **Symptômes** (vécus via UI, alice) : liste devices vide, page Builds
  vide, création de device impossible (403) — alors que l'API répond 200
  partout et que la base contient les devices.
- **Chaîne complète** (constatée dans les logs serveur) :
  1. session expirée → `user-info` 401 → `refresh` 400 → auto-logout ;
  2. `session::logout()` appelle `org::clear()` → la dernière org
     sélectionnée (`pnex.org`) est **purgée** du localStorage ;
  3. au re-login, `org::restore()` retombe sur `memberships.first()` — le
     code suppose que la première org est « l'org personnelle JIT » ;
  4. sauf que `user-info` liste les memberships **sans `ORDER BY`**
     (`user_info.rs:51`) → ordre arbitraire Postgres, constaté `2, 4, 5, 1`
     → la première org est « Hack Co » où alice est **viewer** (0 device,
     création interdite).
- **Correction proposée** (petit commit, non appliqué) :
  - backend : `order_by_asc(organizations::Id)` sur la requête memberships
    de `user-info` — l'org personnelle JIT (créée à la première connexion,
    plus petite id en pratique) sort en premier, ordre déterministe garanti ;
  - front : le fallback de `restore()` préfère la première membership dont
    le rôle n'est **pas** `viewer` — robuste même si l'ordre change.
- **Fichiers** : `crates/pnex-backend/src/controllers/user_info.rs:51`,
  `crates/pnex-frontend/src/state/org.rs:30`,
  `crates/pnex-frontend/src/state/session.rs:52`.
- **Statut** : ✅ résolu (2026-08-17, itération UI Phase 6) — backend :
  `order_by_asc(OrgId)` sur les memberships de `user-info` (org personnelle
  JIT d'abord, ordre déterministe) ; front : le fallback de `org::restore()`
  préfère la première membership non-viewer.

### O2 — `loco start` seul ne drive pas la queue : builds bloqués `queued`

- **Symptôme** : `POST /build-firmware` → 201, le record reste `queued`
  indéfiniment, rien ne se passe côté UI (polling qui tourne dans le vide).
- **Cause** : `loco start` = ServerOnly ; sans `--server-and-worker`,
  personne ne consomme `pg_loco_queue`.
- **Corrigé** (commit `81c37a2`) : `task dev` → `dev:backend` lance
  `--server-and-worker` ; l'ancien comportement devient
  `dev:backend:server-only`. Rappel déploiement : le process prod doit être
  lancé avec worker (déjà consigné dans `firmware-build.md`).
- **Statut** : ✅ résolu.

### O3 — Auto-build proposé pour les devices custom (`custom_sensor`, `custom_device`)

- **Symptôme** : build lancé depuis l'UI sur un device custom → échec
  immédiat (~8 ms, « projet introuvable ») : le workspace firmware ne
  contient que `soil_sensor`, `4_chan_relay`, `tft_dev`
  (`pipeline.rs:259` vérifie `{project}/platformio.ini`).
- **Parité Django** : le POC ne filtre pas non plus — le
  `predefined_device_name` part tel quel au job k8s, qui échoue pareil.
  Comportement backend conforme ; échec propre, file saine.
- **Amélioration UX** (directive déjà consignée `firmware-build.md` §3) :
  ne pas proposer « Compiler maintenant » à l'enregistrement d'un
  `custom_*` ni dans le formulaire Builds ; afficher à la place le
  **snippet de configuration** du code source pour guider l'utilisateur.
- **Statut** : ✅ résolu (2026-08-17, itération UI Phase 6) — le wizard
  n'a pas d'étape WiFi pour les custom (écran token + script Python
  publisher interpolé à la place) et le bouton « Recompiler » de la liste
  est masqué quand `allow_dynamic_measurements`.

### O4 — Raison d'échec d'un build invisible dans l'UI

- **Symptôme** : badge `failed` sans explication ; la cause ne vit que
  dans les logs serveur (tracing worker). `build_records` n'a pas de
  colonne d'erreur, et le front n'a rien à afficher.
- **Pistes** : colonne `last_error` sur le record (texte borné — le
  pipeline capture déjà la queue des 30 dernières lignes) exposée dans le
  DTO ; et/ou logs builds → OpenObserve (différé existant, recoupe).
- **Statut** : à traiter.

### O5 — Bruit `/_dioxus?build_id=0` en build debug servi par Loco

- Le bundle front **debug** (`task dev:frontend`, `dx build` sans
  `--release`) embarque le client hot-reload qui interpelle `/_dioxus` en
  boucle (~2 req/s) ; le fallback statique de Loco répond `index.html` 200
  à chaque fois.
- **Impact** : bruit dans les logs serveur, requêtes inutiles. Sans
  gravité.
- **Pistes** (priorité basse) : filtrer le chemin côté serveur, ou
  désactiver le client hot-reload dans les builds debug hors `dx serve`.
- **Statut** : noté, priorité basse.

### O6 — Résidus de test dans la base dev

- Org 1 accumule 66 devices dont des `seed-*` (métadonnée
  `{"seeded": true}`) ; les orgs « E2E Org » / « E2E Org 2 » (4, 5)
  restent d'un e2e Phase 6. Sans impact fonctionnel, mais fausse les
  démos et le `device_count` de `user-info`.
- **Piste** : purge SQL ciblée ou reset db dev.
- **Statut** : noté, cosmétique.
