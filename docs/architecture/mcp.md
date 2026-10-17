# Serveur MCP (D245–D250)

> Statut : **M1 livré** (2026-10-17) : endpoint intégré, 294 outils sur
> 421 opérations, table d'exposition gardée, validé par les tests backend
> et l'e2e Playwright à travers l'edge TLS.

## 1. Contexte

D245 fait du spec OpenAPI (`docs/contracts/openapi.json`) la source d'un
serveur MCP : un client LLM (Claude Code, Claude Desktop, tout client MCP)
pilote PneX avec le compte de l'utilisateur, sans rien de plus que ses
droits. Les routes destructrices ou physiques ne sont pas exposées (D123,
D144).

## 2. Décisions

- **D246 — MCP intégré au backend.** `POST /api/v1/mcp`, transport
  Streamable HTTP en JSON (pas de flux SSE, pas de session) : `initialize`,
  `ping`, `tools/list`, `tools/call` ; une notification répond 202. Pas de
  binaire à distribuer, pas de second déploiement : l'endpoint suit la
  version du serveur. Protocole 2025-06-18 (2025-03-26 et 2024-11-05
  acceptés).
- **D247 — Un outil = une opération du spec, rejouée en interne.** Les
  outils sont générés au premier appel depuis le spec (nom =
  `operationId`, description = résumé + description, schéma d'entrée =
  paramètres de chemin, de requête et d'en-tête + `body`, schémas
  référencés embarqués en `$defs`). Un appel est rejoué sur le routeur de
  l'application avec l'`Authorization` de l'appelant : mêmes extracteurs,
  mêmes gardes de rôle, mêmes validations, mêmes 409. Le MCP n'a aucun droit
  propre. `X-Org-Id` est un argument ; à défaut, celui de la connexion MCP.
  Erreur du handler → résultat `isError` portant `HTTP <code>: <ErrorDetail>`.
- **D248 — Table d'exposition explicite, gardée.**
  `crates/pnex-backend/src/services/mcp/exposure.rs` : chaque opération est
  soit dans `EXPOSED`, soit dans `EXCLUDED` sous un motif. Motifs actuels :
  destructif (toute suppression, purge, archivage), physique (commandes
  device, OTA, écriture de contrôle, réglages caméra, actions d'ontologie
  qui déclenchent des flows), runtime (deploy/start/stop/restart/rollback),
  firmware (build), secret (création ou mise à jour de clés, mots de passe,
  jetons), accès (membres et rôles), sortant (notification réelle, lien
  public), validation humaine (approbation de version), assistant
  (conversations), plateforme (écritures d'administration), binaire (corps
  non JSON), authentification et canaux device/agent, l'endpoint MCP
  lui-même.
- **D249 — Aucun secret dans une sortie d'outil.** Les champs
  `device_token`, `share_token`, `api_key`, `password`, `auth_secret`,
  `secrets`, `encryption_key` et les jetons OAuth sont masqués
  (`[redacted]`) dans toute réponse, quelle que soit la route (R4, R16).

## 3. Harnais (règle permanente)

Toute route ajoutée entre dans la table **dans le même commit** que son
annotation `#[utoipa::path]` : exposée, ou exclue sous un motif. Gardes
bloquantes (`crates/pnex-backend/tests/mcp.rs`) :

- `every_operation_is_exposed_or_excluded_once` : couverture exacte du spec
  (ni oubli, ni doublon, ni nom périmé) ;
- `exposed_operations_are_safe_and_json` : aucun `DELETE` exposé, aucune
  écriture exposée dont l'`operationId` contient un mot destructif ou
  physique (`delete`, `purge`, `ota`, `deploy`, `stop`, `commands`,
  `secrets`…), corps de requête et de réponse 2xx en JSON ;
- `mcp_serves_tools_with_the_callers_rights` : poignée de main, liste,
  écriture puis lecture, refus 403 d'un non-membre, jeton device masqué,
  outil exclu refusé ;
- tests unitaires du module : schémas d'entrée autonomes (aucune `$ref`
  pendante), masquage en profondeur ;
- e2e `e2e/tests/mcp.spec.ts` : même parcours à travers l'edge TLS.

Exposer une opération auparavant exclue est une décision : elle se justifie
dans le commit, jamais pour faire taire une garde.

## 4. Connexion : une seule fois (D250)

- **D250 — Le client MCP se connecte lui-même (OAuth 2.1).** Un appel sans
  jeton répond 401 avec `WWW-Authenticate: Bearer resource_metadata=…` ;
  `GET /.well-known/oauth-protected-resource/api/v1/mcp` (RFC 9728) désigne
  Rauthy comme serveur d'autorisation. Le client ouvre le navigateur une
  fois (code + PKCE S256, client public `pnex`, retour en boucle locale
  `http://localhost:33418/callback`), puis rafraîchit seul : le refresh
  token s'ouvre dans la dernière minute de l'access token (1 h) et vit 48 h,
  renouvelé à chaque usage. On ne se reconnecte que si le client est resté
  inutilisé plus de 48 h.
- Rauthy exige que la ressource RFC 8707 envoyée par le client figure dans
  `allowed_resources` du client `pnex` ; le jeton porte alors
  `aud = ["pnex", "<origine>/api/v1/mcp"]` (accepté tel quel par le
  backend). `deploy/edge/apply-edge.sh` pose l'URI de retour et la
  ressource à chaque `task app:up` ; **les installations `pnex-deploy`
  doivent recevoir les deux mêmes réglages.**
- Validé par l'e2e `mcp oauth` (découverte, connexion navigateur, échange
  du code, fenêtre de rafraîchissement).

```sh
claude mcp add --transport http --client-id pnex --callback-port 33418 \
  pnex https://<hôte>/api/v1/mcp
```

Sans edge HTTPS (dev nu), pas de découverte : on passe
`--header "Authorization: Bearer <jeton>"` comme avant.

## 5. Limites connues (M2)

- **Liste lourde** : 294 outils, ~340 Ko de `tools/list` (les schémas de
  dashboards et widgets pèsent ~17 Ko chacun). Un client qui charge les
  outils à la demande s'en accommode ; sinon, M2 : filtrage par tag.
- Réponse plafonnée à 4 Mio ; au-delà, l'outil demande de restreindre la
  requête (pagination).
- Le rejeu ne porte pas l'adresse du client : sans effet aujourd'hui, les
  routes sous rate-limit (oauth2, enrôlement, liens publics) étant exclues.
