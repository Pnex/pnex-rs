# Contrats d'API DRF capturés (Phase 0)

Source : projet Django `pnex-server`. Ces contrats asservissent le comportement
Rust (tests de parité, migration.md §4).

## Contenu

- `*.http` — exemples de requêtes copiés depuis `pnex-server/requests/`
  (émis par les développeurs contre l'API réelle).
  ⚠️ `emqx.http` référence un endpoint `/api/v1/emqx/authn` **qui n'existe plus**
  dans le code Django — héritage, à ne pas reproduire.
- Inventaire complet des endpoints + payloads requête/réponse :
  **`../phase0/api-rest.md`** (source principale, avec fichier:line Django).
- Protocoles WebSocket (ingestion, actuateur, dashboards, evaluate, builds) :
  **`../phase0/ws-channels-crypto.md`**.
- Schéma de sortie : l'API DRF peut être interrogée via `GET /schema/`
  (drf-spectacular OpenAPI) sur une instance vivante — à exporter vers
  `openapi.yaml` ici si besoin d'un diff automatisé.

## Règles de parité retenues

1. Pas de pagination DRF : les listes REST sont des **tableaux JSON bruts** ;
   seuls `/metrics/` et `/live-metrics/` wrappent `{"count", "results"}`.
2. Codes de fermeture WS device : 4001-4008 (détail dans ws-channels-crypto.md §2.1).
3. Réactivation implicite : `POST /api/v1/devices/` sur device inactif → 200 ;
   création → 201 ; déjà actif → 400.
4. `PUT/PATCH /devices/{id}` : **metadata uniquement** (400 sinon).
5. Erreurs de build : 404 device, 403 quota, 429 intervalle min, 500 soumission.
6. Les corps d'erreur sont soit `{"detail": "..."}` (DRF), soit `{"error": "..."}`
   selon les vues — la parité exacte est consignée dans api-rest.md.
