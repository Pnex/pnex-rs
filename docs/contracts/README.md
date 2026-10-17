# Contrats d'API capturés (Phase 0)

Source : l'ancienne stack (repo `pnex-server`, aujourd'hui retiré). Ces
contrats asservissent le comportement Rust (tests de parité).

## Contenu

- `*.http` — exemples de requêtes copiés depuis `pnex-server/requests/`
  (émis par les développeurs contre l'API réelle).
  ⚠️ `emqx.http` référence un endpoint `/api/v1/emqx/authn` **qui n'existe plus**
  dans le code de l'ancienne stack — héritage, à ne pas reproduire.
- Les rapports d'exploration d'origine (inventaire complet des endpoints
  + payloads requête/réponse, protocoles WebSocket) ont été supprimés —
  les règles ci-dessous et les exemples `.http` font foi.
- `openapi.json` — spec OpenAPI 3.1 de l'API, **généré** depuis les
  annotations `#[utoipa::path]` des handlers et les `ToSchema` des DTO
  (`crates/pnex-backend/src/openapi.rs`), source des SDK. Jamais édité à la
  main : `task openapi` le régénère. Gardes (`crates/pnex-backend/tests/openapi.rs`) :
  fichier à jour, et toute route documentée ou listée dans
  `tests/fixtures/openapi_pending.txt` — liste qui ne fait que décroître
  (une route neuve s'annote, elle ne s'ajoute pas à la liste). Hors
  périmètre : WebSockets (`/ws/*`), endpoints internes (`/internal/*`),
  sondes loco (`/_*`). Non servi en HTTP (pas de Swagger UI).

## Règles de parité retenues

1. Pas de pagination dans l'ancienne stack : les listes REST sont des **tableaux JSON bruts** ;
   seuls `/metrics/` et `/live-metrics/` wrappent `{"count", "results"}`.
2. Codes de fermeture WS device : 4001-4008.
3. Réactivation implicite : `POST /api/v1/devices/` sur device inactif → 200 ;
   création → 201 ; déjà actif → 400.
4. `PUT/PATCH /devices/{id}` : **metadata uniquement** (400 sinon).
5. Erreurs de build : 404 device, 403 quota, 429 intervalle min, 500 soumission.
6. Dans l'ancienne stack, les corps d'erreur étaient soit `{"detail": "..."}`, soit
   `{"error": "..."}` selon les vues.
