# Assistant IA intégré (2026-09-06)

> **Statut : IMPLÉMENTÉ (tranche v1)** — chat multi-tools, connecteur par
> org, lecture OpenObserve, création/modification de **drafts** de flows.
> Détail des décisions : `docs/inventory.md` D20 ; plan de conception
> validé en session (A1–A5).
>
> **2026-10-01 (secrets.md D116, lot S7)** : le connecteur par org et les
> `PNEX_AI_*` sont remplacés par des **fournisseurs LLM en CRUD**
> (`llm_providers`, clé dans le coffre). A3 et A5 sont caducs ; §2, §3, §6,
> §7 décrivent l'état actuel.

## 0. Décisions actées (2026-09-06)

| # | Décision |
|---|---|
| A1 | Tranche complète v1 : chat + connecteur par org (UI+DB) + lecture O2 + flows draft |
| A2 | UI = panneau flottant global (drawer latéral, pattern `DebugDrawer`, monté dans `pages/shell.rs`) |
| A3 | ~~Config : env `PNEX_AI_*` > connecteur org > désactivé~~ → remplacé par D116 : défaut de l'org > défaut plateforme > non configuré ; kill-switch `PNEX_AI_ENABLED=false` conservé |
| A4 | Devices **read-only** : l'agent ne pousse aucune commande (frontière D17 propre) |
| A5 | Table `ai_connectors` par org ; drop des 3 colonnes mortes `user_profiles.llm_*` (« modèle sans copies ») |

## 1. Architecture

```
UI (AssistantPanel : bouton flottant + drawer, toutes pages)
   │ POST /api/v1/ai/chat {messages, language, page}
   ▼
controllers/ai.rs (OrgContext) ──► services/ai/
   ├─ config.rs   : kill-switch + résolution env > org > désactivé
   ├─ provider.rs : Anthropic natif | OpenAI-compatible (reqwest, hand-rolled)
   ├─ agent.rs    : boucle bornée (≤6 appels, max_tokens 4096, timeout 90 s)
   ├─ tools.rs    : REGISTRE FERMÉ de 10 outils — unique chemin d'exécution
   └─ context.rs  : prompt système + bundle org (devices, pins, séries O2, flows)
```

**Garde-fous** (invariant structurel, testé) :
- la surface exécutable par le LLM est **exactement**
  `services/ai/tools::execute` — un `match` fermé sur 10 outils ;
  pas de deploy, pas de suppression, pas de commande device, aucun outil
  générique (http/sql) ;
- les outils d'écriture re-vérifient `can_write` (chat ouvert aux viewers,
  écriture owner/admin) ;
- `create_flow`/`update_flow` passent `pnex_core::validate_graph` avant
  persistance ; `validate_calc_expression` permet l'auto-correction ;
- le chemin `flow_supervisor` / `POST /flows/{id}/deploy` n'est référencé
  nulle part dans le module `ai` — **enregistrer ≠ déployer** (le déploiement
  reste un geste humain, versionné, dans l'éditeur).

## 2. Fournisseurs LLM (D116, 2026-10-01)

Table `llm_providers` (migration `m20261001_000048`) : `org_id` (NULL =
plateforme), `name` (unique par propriétaire), `kind` (`anthropic` |
`openai_compat`), `base_url` (requise pour openai_compat, racine de
version `…/v1`, sans slash final), `model`, `secret_id` (clé API = référence
au coffre, secret dédié `llm/<nom>/api_key`), `is_default` (un seul par
propriétaire, index partiels).

Résolution (`services/ai/providers.rs::effective`) : fournisseur par
défaut de l'org, sinon fournisseur par défaut de la plateforme (géré par
l'admin plateforme dans /system), sinon **non configuré** (le drawer
l'indique). La clé est déchiffrée en mémoire à chaque appel. Le défaut
plateforme est **visible et utilisable par toutes les orgs**, sa clé leur
reste illisible (la liste de l'org le montre sans référence de clé).

Plus aucune configuration LLM en variable d'environnement : seul le
kill-switch `PNEX_AI_ENABLED=false` subsiste (chat 403 + UI masquée, sans
requête DB). Reprise au boot : chaque ligne `ai_connectors` devient un
fournisseur de l'org (par défaut si l'org n'en a pas), sa clé passe au
coffre, la ligne est supprimée ; la table sera supprimée par une migration
ultérieure.

## 3. API

| Endpoint | Accès | Effet |
|---|---|---|
| `GET /api/v1/ai/status` | membre | `{enabled, configured, source (org\|platform), provider_name, provider, model}` |
| `GET /api/v1/ai/providers` | membre | fournisseurs de l'org + défaut plateforme (`platform: true`, sans référence de clé) |
| `POST /api/v1/ai/providers`, `PUT`/`DELETE /{id}` | owner/admin | clé : `{"value"}` (secret dédié) ou `{"secret_id"}` (secret de l'org) ; `PUT` sans `api_key` = conserver ; 409 `llm-provider-name-taken` |
| `POST /api/v1/ai/providers/{id}/test` | owner/admin | ping one-shot, `{ok, latency_ms, error}` |
| `/api/v1/system/ai/providers[/{id}[/test]]` | admin plateforme | mêmes opérations sur les fournisseurs plateforme |
| `POST /api/v1/ai/chat` | membre | boucle d'agent ; `{answer, tool_trace}` (trace avec `flow_id` pour le deep-link éditeur) |

Chat **sync** en v1 (boucle multi-tours : SSE n'apporte que le texte final ;
client front non-streaming ; budget borné). SSE différé en v2.

## 4. Outils (registre fermé)

`list_devices` · `get_device_pins` · `list_flows` · `get_flow` ·
`query_telemetry` (réutilise `visualization::series_points`, anti-injection
promwrap, `available:false` dégradé) · `describe_node_types` (statique,
cite les 8 variantes `FlowNodeKind` + pipeline canonique) ·
`validate_flow_graph` · `validate_calc_expression` · `create_flow` ·
`update_flow` (drafts uniquement, note « par l'assistant IA »).

Écriture = `services::flow::{create_flow, append_version}` — point
d'écriture **unique** partagé avec `controllers/flows.rs` (extraction de la
tranche refactor ; comportement HTTP identique, tests existants verts).

## 5. Prompt système

Reconstruit à chaque tour (`context.rs`) : rôle + garde-fous explicites
(« ne promets jamais deploy/suppression/commandes »), règles moteur
(`device_payload_key`, `etl_metric_name`, pipeline `[inject]→[device]→[calc]→[metric]`),
bundle org vivant (devices ≤50 + pins, séries O2 ≤30, flows ≤20), contexte
de page, langue (fr défaut, en).

## 6. UI (Dioxus)

- `components/assistant.rs` — bouton flottant bas-droite (visible si
  `enabled && configured`) + drawer (pattern `DebugDrawer`) : bulles,
  trace d'outils repliable, carte « Ouvrir dans l'éditeur » (deep-link via
  `state::flows::OPEN_FLOW`).
- `components/llm_providers.rs` — liste + formulaire en ligne, monté dans
  `OrgDetail` (fournisseurs de l'org, défaut plateforme en lecture seule)
  et dans /system (fournisseurs plateforme). Clé = `SecretField` (saisie
  seule côté plateforme) ; boutons Tester / Modifier / Supprimer.
- i18n : clés `ai-*` dans les deux `.ftl` (test de parité).

## 7. Variables d'environnement

| Variable | Rôle | Défaut |
|---|---|---|
| `PNEX_AI_ENABLED` | kill-switch global | `true` |

Les anciennes `PNEX_AI_PROVIDER`, `PNEX_AI_BASE_URL`, `PNEX_AI_API_KEY` et
`PNEX_AI_MODEL` sont **retirées sans import** (D116) : un déploiement qui
les posait doit créer le fournisseur plateforme dans /system.

## 8. Tests

- **Unitaires** : précédence config, mapping wire des deux protocoles
  (fixtures JSON, regroupement tool_result Anthropic, arguments string→Value
  OpenAI), registre == ensemble autorisé (aucun nom deploy/delete/command),
  `execute` refuse les noms hors registre.
- **Intégration** (`tests/ai.rs`, mock LLM HTTP local) : kill-switch 403
  sans requête LLM ; CRUD fournisseurs (clé au coffre, défaut unique,
  validations, 409 nom) ; viewer 403 et /system réservé ; défaut org >
  défaut plateforme (le ping va au mock) ; tour complet → **flow créé draft,
  0 deploy, tool_result dans le 2e appel** ; outil interdit → ok:false ;
  401 → 502 actionnable ; sans fournisseur → non configuré, chat 400 ; borne d'itérations.
  Reprise `ai_connectors` → `llm_providers` : `tests/secrets.rs`.
