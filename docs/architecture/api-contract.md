# Contrat d'API HTTP : spec OpenAPI et harmonisation (D238–D245)

> Statut : **H0–H5 livrés et validés** (2026-10-17, non commité) :
> `cargo test --workspace` vert (165 binaires), 8 gardes du spec vertes,
> Redocly valide, SDK TS (421 opérations, `tsc --strict`) et Rust (hors
> bug connu du générateur sur les uploads binaires), e2e Playwright
> 177 passés / 0 échec sur image `pnex-server:dev` (33 ignorés : matériel,
> géo live, jetons de test).

## 1. Contexte

Le spec `docs/contracts/openapi.json` est généré depuis le code
(`#[utoipa::path]` sur chaque handler, `ToSchema` sur les DTO,
`crates/pnex-backend/src/openapi.rs`). Il sert de source aux SDK (TypeScript
et Rust validés : génération + compilation + appels réels) et, plus tard, à
un serveur MCP (les `operationId`, résumés et descriptions deviennent des
outils pour un LLM).

L'audit du spec complet (2026-10-17) montre une API fonctionnelle mais
hétérogène :

| Axe | Constat |
|---|---|
| Pagination | 33 listes `Paginated` (D14), 39 tableaux bruts, 10 `{count, results}`, 2 enveloppes avec `available` |
| Erreurs 400 | 113 maps de champs `{champ: token}`, 52 `ErrorDetail`, 17 non typées ; souvent deux formes sur la même route |
| Identifiants | 62 `int64`, 47 UUID déclarés, 133 `string` sans format (souvent des UUID) |
| Dates | 162 champs `string` sans `format: date-time` |
| Enums | aucun : statuts, modes et rôles en chaînes libres ; un rôle renvoyé capitalisé (`"Owner"`) |
| Langue | messages d'erreur en français dur dans plusieurs contrôleurs ; 136 schémas sur 564 décrits en français |
| Verbes | lectures en `POST` (`cases/{id}/signals`, `expand`, `tracks`) ; `PUT`/`PATCH` mélangés pour une même sémantique |

On est en beta (pas de données à préserver, rupture permise) : on harmonise
maintenant, en une rupture de contrat (`pnex_api_contract::CONTRACT` 2 → 3).

## 2. Décisions

- **D238 — Le spec OpenAPI généré est le contrat public.** Jamais écrit à la
  main : `task openapi` le régénère, `tests/openapi.rs` vérifie qu'il est à
  jour et que **toute** route est documentée (liste d'attente vide, elle ne
  peut que décroître). Hors périmètre : `/ws/*` (WebSocket), `/internal/*`
  (serveur à serveur), sondes loco `/_*`. Non servi en HTTP (pas de Swagger
  UI). `X-Org-Id` est un **paramètre d'en-tête obligatoire** (pas un schéma
  de sécurité : les générateurs n'appliquent qu'un schéma d'un ET).
- **D239 — Une seule forme d'erreur : `ErrorDetail`.**
  `{"error": "<code>", "description": "<anglais canonique>", "errors": {…}}`
  pour **tout** statut non 2xx d'une route publique.
  - Erreur de champ : code `field-invalid`, `errors.fields = {"<champ>": "<jeton>"}`
    (jetons machine inchangés : `required`, `max_length:255`…). Plus de corps
    `{champ: jeton}` nu.
  - Violations de flow : code `flow-invalid`, `errors.violations = [...]`.
  - Interpolation : `errors.args` (inchangé).
  - Aucun texte français dans une réponse : un message pré-rendu devient un
    code de `err_codes::ALL` + clé `err-<code>` dans les deux `.ftl`.
  - Codes en kebab-case (les codes OTA `ota_in_progress`… sont renommés).
  - Aucune exception : les routes consommées par le firmware ou l'agent
    edge (`/api/v1/agent/*`, `/api/v1/ota/firmware/*`,
    `/api/v1/download/firmware/*`) suivent la même règle ; leur consommateur
    (`firmware/lib/pnex`, agent edge) est adapté dans le même lot (décision
    user 2026-10-17 : rupture totale permise, tests refaits ensuite).
- **D240 — Toute liste est paginée (D14 strict).** Une réponse dont le corps
  est un tableau JSON nu est interdite : collection → `Paginated<T>`
  (`count`, `next`, `previous`, `results`, `limit`/`offset`), y compris les
  versions, les sous-collections et les petits catalogues fixes (cohérence
  du SDK > économie). Un résultat calculé qui n'est pas une collection
  (chemin, graphe d'impact) est un objet nommé. L'indisponibilité d'un
  backend (OpenObserve) n'est plus un champ `available` : erreur 503 codée.
- **D241 — Types précis.** Identifiant de chemin typé (`int64` ou `uuid`
  déclaré) ; dates `format: date-time` (RFC 3339 sur le fil, inchangé) ;
  ensembles fermés (statuts, modes, rôles, phases) en `enum` Rust sérialisé
  en minuscules ; rôles toujours en minuscules. Les colonnes de base peuvent
  évoluer pour porter ces types (enum Postgres, `timestamptz`) si un DTO le
  demande.
- **D242 — Verbes.** Lecture = `GET`. Un `POST` de lecture n'est admis que
  pour une requête à corps complexe, nommée `…/query` ou `…/search`.
  `PATCH` = mise à jour partielle, `PUT` = remplacement complet ; une mise à
  jour partielle exposée en `PUT` passe en `PATCH`.
- **D243 — Descriptions du contrat en anglais.** Résumés, descriptions et
  documentation des champs des DTO exposés (y compris les commentaires de
  `pnex-core`) sont en anglais. Exception assumée à « ne pas repasser sur
  l'existant rien que pour la langue » : ces commentaires sont publiés dans
  le contrat.
- **D244 — Rupture versionnée.** `CONTRACT` passe à 3 avec le lot H1 ; le
  front embarqué bascule dans le même commit (porte de compatibilité au
  boot). Le spec porte la version du workspace.
- **D245 — MCP : le spec est la source, l'exposition est filtrée.** Un futur
  serveur MCP se génère depuis le spec, avec le jeton de l'utilisateur (ses
  droits, pas plus). Les routes destructrices ou physiques (OTA, commandes
  device, deploy/stop de flow, purge, suppressions) ne sont pas exposées par
  défaut : liste d'autorisation explicite, cohérente avec les consignes de
  l'assistant (D123, D144). PRD dédié avant implémentation.

## 3. Lots

| Lot | Contenu | Validation |
|---|---|---|
| H0 ✅ | Socle : `field_error`/`fields_error`/`violations_error` (`controllers/mod.rs`), codes `field-invalid` + `<document>-invalid` (+ `.ftl`), `ApiError::fields()`/`violations()` côté front, `CONTRACT` = 3 | tests unitaires front + backend |
| H1 ✅ | Erreurs (D239) par domaine, codes OTA en kebab, messages FR → codes ; `CONTRACT` = 3 | tests backend, `error_codes.rs`, `i18n_guard` |
| H2 ✅ | Pagination (D240) par domaine, front et harnais e2e (`e2e/src/api.ts`) adaptés | tests backend + compilation wasm |
| H3 ✅ | Types (D241) : enums, `date-time`, UUID, rôles | spec + SDK régénérés |
| H4 ✅ | Verbes (D242) | tests backend |
| H5 ✅ | Langue (D243) | revue du spec (aucun texte FR) |
| V ✅ | Validation : `task check`, `cargo test --workspace`, SDK TS + Rust régénérés et compilés, **suite e2e Playwright complète** sur une image construite depuis la branche | tout vert |

Le découpage d'exécution est par domaine (une même personne traite H1–H5
sur son domaine, front compris) après H0.

## 4. Exceptions assumées

- **OAuth2** : `POST /oauth2/token` et `/oauth2/refresh` relaient les refus
  de l'IdP dans la forme standard RFC 6749 §5.2 (`{error,
  error_description}`) attendue par tout client OAuth2 ; nos propres
  erreurs de champ y restent `field-invalid`. Pages HTML (`logout`,
  `native`) et redirection `sso` hors JSON.
- **`/resources/{kind}/{id}`** : `id` polymorphe (`int64` pour un device,
  UUID pour un média ou une page), déclaré `string`.
- **Rejets d'extracteur axum** (JSON illisible, valeur d'enum inconnue,
  query/chemin invalide) : réécrits en `ErrorDetail` code
  `request-invalid` par une couche globale
  (`controllers::rejection_to_error_detail`), description = message du
  parseur.
- **Disponibilité partielle dans un résultat calculé** : les champs
  `available` des requêtes multi-séries de dashboards (une série indisponible
  ne fait pas échouer les autres), du résumé d'accueil et des pistes/signaux
  d'enquête (aussi figés dans les instantanés) restent ; une **liste**
  indisponible est une 503 codée (`events-unavailable`,
  `telemetry-unavailable`, `notify-journal-unavailable`,
  `case-reads-unavailable`, `ontology-changes-unavailable`,
  `memory-unavailable`).
- **Résultats classés** (`GET /media/search` `{hits}`, recherche globale
  `{count, q, groups}`) : objets nommés, pas des collections paginées.

## 5. Restes connus (après H5)

- Ensembles fermés encore en chaînes, faute de bénéfice net à ce stade :
  `CaseProposal.kind`/`status`, `SignalItem.kind`, `CaseRead.purpose`,
  `TourDoc.mode`/`TourLink.kind` (dans le document versionné),
  `Device.device_type`, `AgentKey.kind`/`chip_family`,
  `OtaAssignmentState.state`, `IndexState.status`, `MediaStream.capture_on`
  (`server|worker|device:<id>`, non fermé). Codes de violation de documents
  (flow, visite, calque) en snake_case comme les codes de diagnostic
  (`err-<kebab>` côté front, hors `ALL`).
- `o2-not-configured` sert aussi à « OpenObserve injoignable » (502) : nom
  trompeur, à renommer avec le chantier O2.
- Un ordre OTA signé avant la mise à jour pointe l'ancienne URL
  (`/ota/firmware/{device_id}/{version}` → `/ota/firmware/{version}`) :
  404 une fois, redéployer (beta).

## 6. Gardes

- `tests/openapi.rs` : spec à jour + couverture totale, et sur le spec
  généré : aucune réponse 2xx tableau nu (`no_bare_array_responses`), toute
  réponse d'erreur à corps = `ErrorDetail`
  (`error_responses_are_error_detail`), `*_at` en `date-time` et id de
  chemin `int64`/`uuid` (`ids_and_timestamps_are_typed`), aucun texte
  français (`contract_text_is_english`, heuristique). Les verbes (D242) et
  les enums restent à la revue.
- `error_codes.rs` : tout code littéral enregistré (inchangé).
