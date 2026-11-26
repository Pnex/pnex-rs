# Paperclip : lecture du code pour les agents de PneX

**Date :** 2026-11-24 · **Pour :** `docs/architecture/agents.md` (§3.6, §3.9, §3.12, §3.19) · **Parent :** `../agents-ops-correlation.md`

Lecture du code et des docs d'un clone `--depth 1`. Rien n'a été installé, construit ni lancé. Les chemins `path:line` sont relatifs à la racine du clone `~/Documents/shan-perso/bench/paperclip`.

## 1. Identité

| Point | Valeur |
|---|---|
| Dépôt | https://github.com/paperclipai/paperclip · docs https://docs.paperclip.ing/ |
| Commit étudié | `28aa32e7d7ba72d8f83c03ffd45625c096813e6b` (2026-11-24) |
| Licence | MIT, confirmée dans `LICENSE`. Les sous-dossiers sous licence propre sont aussi MIT (`packages/adapters/hermes/LICENSE`, `packages/shared/src/cliplab/LICENSE`). Pas de dossier `ee/`. |
| Stack | Node.js et TypeScript. Express 5, React 19, Vite, Drizzle ORM, PostgreSQL (embarqué en local), S3 en option, `ws` pour le direct. |
| Taille | 202 fichiers de schéma (`packages/db/src/schema/`), 1 442 fichiers dans `server/src/services/`, environ 1,77 M lignes TS dans `server/src`. `heartbeat.ts` fait 4 872 lignes, `heartbeat/queue.ts` 5 649. |
| Activité | Créé le 2026-03-02. 116 k étoiles, 19 k forks, environ 259 contributeurs, numéros de tickets et PR au-delà de 22 750. Poussé le jour même. |

Le produit se décrit comme « l'entreprise » des agents : objectifs, projets, tickets (`issues`), organigramme, budgets, validations d'un « board ». L'unité de travail est le ticket. Un agent travaille par « heartbeats » : des fenêtres courtes, lancées par un réveil.

## 2. Modèle de données

| Objet | Table | Rôle | Réf. |
|---|---|---|---|
| Company | `companies` | Frontière de locataire, équivalent de l'org | `packages/db/src/schema/companies.ts` |
| Agent | `agents` | `adapter_type` + `adapter_config` + `runtime_config` (jsonb), `reports_to` (organigramme), budget mensuel, `permissions` jsonb, `harness_verified_digest` | `agents.ts:17`, `:37`, `:35`, `:43` |
| Réveil | `agent_wakeup_requests` | Demande de run : `source`, `reason`, `payload`, `status`, `coalesced_count`, `idempotency_key` | `agent_wakeup_requests.ts:15`, `:30` |
| Run | `heartbeat_runs` | Un heartbeat : statut, source, ticket, usage, résultat, journal (`log_ref`, `log_sha256`), extraits stdout/stderr, retry, liveness | `heartbeat_runs.ts:31`, `:90` |
| Événement de run | `heartbeat_run_events` | Flux ordonné par `seq` : type, stream, niveau, message | `heartbeat_run_events.ts:18` |
| Session | `agent_task_sessions`, `agent_runtime_state` | Session du harnais par agent et clé de tâche, cumuls de tokens et de coût | `agent_task_sessions.ts:16`, `agent_runtime_state.ts:5` |
| Ticket | `issues` | Tâche, arbre `parent_id`, `assignee_agent_id`, `checkout_run_id`, `execution_run_id`, `execution_policy` jsonb | `issues.ts:26`, `:56`, `:77` |
| Routine | `routines`, `routine_revisions`, `routine_triggers`, `routine_runs` | Tâche récurrente, révisions en ajout seul, déclencheurs `schedule`/`webhook`/`api`, exécutions | `routines.ts:23`, `:67`, `:106`, `:144` |
| Budget | `budget_policies`, `budget_incidents`, `budget_reservations` | Politique par portée, incident de dépassement, réservation avant départ | `budget_policies.ts:5`, `budget_incidents.ts:7`, `accounting.ts:52` |
| Coût | `cost_events` | Une ligne par usage : fournisseur, modèle, tokens, coût, `cost_status`, `pricing_provenance`, `idempotency_key` | `cost_events.ts:11` |
| Validation | `approvals` | Demande au board : `type`, `payload`, `decision_note` | `approvals.ts:6` |
| Outils | `tool_applications`, `tool_connections`, `tool_catalog_entries`, `tool_profiles`, `tool_policies`, `tool_action_requests`, `tool_call_events` | Passerelle MCP gouvernée | `tool_access.ts:85`, `:111`, `:374`, `:420`, `:607`, `:1052`, `:1092` |
| Secret | `company_secrets`, `company_secret_bindings`, `secret_access_events` | Secret par portée, liaison vers une cible et un chemin de config | `company_secrets.ts:10`, `company_secret_bindings.ts:5` |
| Compétence | `company_skills` | Markdown + inventaire de fichiers, `source_type`, `source_ref`, `trust_level` | `company_skills.ts:19` |
| Espace d'exécution | `execution_workspaces` | `mode`, `strategy_type`, `provider_type`, dépôt, branche, nettoyage | `execution_workspaces.ts:15` |
| Livrable | `issue_work_products` | PR, URL, preview liés au ticket et au run | `issue_work_products.ts:18` |
| Droit | `principal_permission_grants` | `principal_type`, `permission_key`, `scope` jsonb | `principal_permission_grants.ts:4` |
| Frein | `automatic_run_brakes` | Disjoncteur par company : `closed`/`open`/`half_open` | `automatic_run_brakes.ts:4` |

Relations clés : un réveil donne au plus un run. Un run porte un ticket et un agent. Un coût porte un run, un ticket, un projet et un objectif. Un dépassement crée un incident, et l'incident crée une validation.

## 3. Exécution des agents

- **Adaptateurs** (`packages/adapters/`) : `claude-local`, `codex-local`, `cursor-local`, `cursor-cloud`, `gemini-local`, `grok-local`, `kimi-local`, `opencode-local`, `pi-local`, `hermes`, `hermes-gateway`, `openclaw-gateway`. S'y ajoutent `process` (commande shell) et `http`. Des adaptateurs tiers s'installent en plugin.
- **Contrat** : `ServerAdapterModule` (`packages/adapter-utils/src/types.ts:508`). Il expose `execute`, `testEnvironment`, `listSkills`/`syncSkills`, `sessionCodec`, `getQuotaWindows`, `listModels`, `getConfigSchema`.
- **Ce que reçoit le harnais** (`types.ts:237`) : `runId`, l'agent, la config, le contexte de tâche, un `AbortSignal`, un `authToken` (JWT de run), les outils et le MCP du run. Il rend ses sorties par `onLog`, `onEvent`, `onUsage`.
- **Ce qu'il rend** (`types.ts:102`) : code de sortie, `timedOut`, usage par modèle, `costStatus` (`reported`, `estimated`, `unpriced`), `pricingProvenance`, `usageBasis` (`per_run` ou `session_cumulative`), `sessionParams` pour la reprise.
- **Départ d'un run** : un réveil entre en file. Le dispatch prend le run suivant quand l'agent a une place libre. Une réservation de budget est prise, puis revérifiée juste avant l'appel au fournisseur (`server/src/services/budget-reservations.ts:32`).
- **Session** : la session du harnais est gardée par agent et par tâche, puis reprise au heartbeat suivant. Une empreinte de config décide si la session reste valide (`server/src/services/heartbeat/session-fingerprint-inputs.ts`). Une config modifiée s'applique au run suivant, jamais au run en cours.
- **Espaces** : `shared_workspace`, `isolated_workspace` ou `operator_branch`, avec la stratégie `project_primary` ou `git_worktree` (`packages/shared/src/types/plugin.ts:732`). Des instantanés chiffrés vont sur S3 et se restaurent dans un nouveau bac. Les bacs distants ont un plafond de 4 h par défaut.
- **Isolation** : les adaptateurs locaux tournent **sans bac** sur l'hôte (`docs/agents-runtime.md` §9). Un mode « native » passe par `packages/paperclip-runner` avec des bacs distants.

## 4. Déclencheurs

- **Sources de réveil** : `timer` (intervalle), `assignment` (ticket assigné), `on_demand` (bouton, API), `automation`. Chaque source s'active par agent dans `runtime_config.heartbeat`.
- **Coalescence** : un réveil sur un agent déjà actif sur le même ticket fusionne dans le réveil en attente. Le compteur `coalesced_count` monte (`server/src/services/heartbeat/queue.ts:4701`). Des index uniques partiels sur `idempotency_key` bloquent les doublons par motif (`agent_wakeup_requests.ts:50` et suivants).
- **Routines** : une routine crée un ticket assigné à un agent, donc un run. Un index unique partiel garde un seul ticket ouvert par routine (`issues.ts:160`).
- **Concurrence d'une routine** (`server/src/services/routines.ts:1994`) : `coalesce_if_active` (défaut), `skip_if_active`, `always_enqueue`.
- **Rattrapage** : `skip_missed` (défaut) ou `enqueue_missed_with_cap`, plafonné à 25 runs (`routines.ts:101`, `:3611`). Un cron plus fréquent qu'une heure n'est jamais rattrapé. Un projet en pause saute ses ticks sans les rejouer.
- **Webhook** : signature `hmac_sha256` ou `bearer`, fenêtre de rejeu, rotation du secret. `Idempotency-Key` avec une portée `trigger` ou `routine`. Un en-tête `X-Paperclip-Source-Run-Token` (JWT de run) rattache le nouveau run au run appelant (`docs/api/routines.md`, « Verified creation provenance »).
- **Ordre de la file** (`server/src/services/heartbeat/queued-run-dispatch-order.ts`) : d'abord les runs demandés par une personne, puis le travail d'agent, puis la reprise. Un départ sur quatre va au plus ancien run en attente depuis plus de 60 min. La reprise garde un départ sur quatre.

## 5. Contrôle

**Budgets** (`server/src/services/budgets.ts`) :
- Une politique vise la company, un agent ou un projet. Elle porte une métrique (`billed_cents`), une fenêtre, un montant, un seuil d'alerte (`warn_percent`, 80) et un arrêt dur.
- `unpriced_usage_policy` vaut `block` par défaut : un usage sans prix bloque le départ (`budgets.ts:244`).
- Une réservation `held` est prise à l'admission, puis `settled` ou `released` (`accounting.ts:52`). La porte finale relit la réservation et la politique sous verrou.
- L'arrêt dur ouvre un incident, met la portée en pause et arrête le travail en cours (`budgets.ts:256`, `heartbeat/budget-cancellation.ts`). Une validation `budget_override_required` permet la reprise.

**Validations** : quatre types (`packages/shared/src/constants.ts:864`) : `hire_agent`, `approve_ceo_strategy`, `budget_override_required`, `request_board_approval`. Un ticket peut aussi porter une `execution_policy` avec des étapes `review` et `approval`. Le réviseur exclut l'exécutant (`docs/guides/execution-policy.md`).

**Passerelle d'outils** (`doc/MCP-ACCESS-GOVERNANCE.md`) :
- Chaîne : application → connexion MCP → catalogue → profil (ce que l'agent voit) → politique (si cet appel passe maintenant).
- Le catalogue classe chaque outil `read`, `write` ou `destructive` d'après les annotations MCP. Un outil d'écriture nouveau ou modifié entre en quarantaine (`tool_access.ts:406`).
- Politiques : `allow`, `block`, `require_approval`, `rate_limit`, `trust_rule`. Le refus gagne toujours.
- `require_approval` rend 409 `approval_required` avec un `actionRequestId`. La demande garde une empreinte canonique des arguments. Au retour, des arguments changés rendent `signed_arguments_mismatch`.
- Une validation peut devenir une `trust_rule` : même acteur, même outil, même empreinte d'arguments, même schéma, avec un seuil et une expiration. Un schéma qui change annule la règle.

**Droits** : `principal_permission_grants` (clé de permission + portée jsonb). Les compétences sont **permises par défaut** à tout agent authentifié, sauf politique explicite (`doc/company-skill-policy.md`). Invariants non réglables : source distante épinglée par commit ou hash, chemins confinés, refus du « fetch-and-exec », audit obligatoire.

**Secrets** : portée company, utilisateur, agent, projet ou plugin. Fournisseurs `local_encrypted` ou référence externe (`aws_secrets_manager`). Une liaison donne `target`, `config_path`, `version_selector` et une livraison `env`, `api` ou les deux (`docs/api/secrets.md`). Chaque lecture laisse un `secret_access_events`.

**Injection** : un filtre regex lit chaque résultat d'outil. Un classifieur LLM optionnel peut le remplacer. La doc dit qu'il ne garantit rien (`doc/prompt-injection-prevention.md`).

**Frein automatique** (`server/src/services/recovery/automatic-run-brake.ts:8`) : il vise seulement les runs automatiques (reprises, relances), jamais ceux d'une personne. Il s'ouvre à 80 % d'échecs avant démarrage sur 10 min (12 runs au moins), ou après 5 refus d'affilée du fournisseur. Ensuite une sonde part chaque minute. Deux succès rouvrent la file par paliers.

## 6. Observabilité et évaluation

- **Journal de run** : stocké hors de la ligne de run (`log_store`, `log_ref`, `log_bytes`, `log_sha256`), en segments S3 ou sur disque (`doc/run-evidence-storage.md`). La ligne garde des extraits stdout et stderr. Les événements structurés restent en base, ordonnés par `seq`.
- **Coût** : `cost_events` sépare le coût annoncé, le coût estimé et l'usage sans prix. Chaque ligne porte la provenance du tarif et une clé d'idempotence. Un usage sans prix compte comme inconnu, jamais comme gratuit.
- **Direct** : statut des agents, runs, coûts et activité poussés par WebSocket.
- **Évaluations** : `doc/evals.md` décrit des campagnes de qualification du produit (harnais × fournisseur × cible), pas des évaluations des agents d'un client. Une taxonomie d'échec utile : produit, comportement du modèle, notation, infrastructure.
- **Retour humain** : pouce haut ou bas sur un commentaire d'agent, raison facultative, consentement au partage. Chaque vote garde un paquet de contexte rejouable (`docs/feedback-voting.md`, `feedback_votes.ts:5`).
- **Gardien de tâche** : un agent « watchdog » se réveille quand un arbre de tickets s'arrête et juge si l'arrêt est légitime (`doc/TASK-WATCHDOG.md`).

## 7. Pièges et limites

Tirés du code :
- Les adaptateurs locaux tournent sans bac sur l'hôte, avec les identifiants de l'hôte.
- Les règles de confiance acceptent seulement l'empreinte exacte. Les limites de débit ne s'additionnent pas entre politiques.
- Le point MCP propre de Paperclip (`/mcp`) n'est pas soumis aux profils ni aux politiques.
- La complexité est extrême : environ 80 colonnes sur `heartbeat_runs`, des dizaines de travaux « idle » de reprise (`doc/idle-*.md`). La reprise de pannes domine le code.

Tirés des tickets GitHub :
- Pas de frein contre une boucle sans progrès. L'arrêt de budget arrive après le gaspillage : https://github.com/paperclipai/paperclip/issues/390
- Le réveil par minuterie coûte à vide. Les tickets « en cours » sans fin créent des réveils de reprise sans borne : https://github.com/paperclipai/paperclip/issues/3882 , https://github.com/paperclipai/paperclip/issues/3402
- La session reprise grossit sans fin : https://github.com/paperclipai/paperclip/issues/3813 . La charge de reprise dépasse la taille d'un argument et tous les adaptateurs locaux échouent (`E2BIG`) : https://github.com/paperclipai/paperclip/issues/13959
- Blocages silencieux de reprise et de relais vers le réviseur : https://github.com/paperclipai/paperclip/issues/14600 , https://github.com/paperclipai/paperclip/issues/13532 , https://github.com/paperclipai/paperclip/issues/15461
- Des runs en attente de reçu de coût bloquent la reprise du budget : https://github.com/paperclipai/paperclip/issues/15649
- `timeoutSec: 0` laissait un run pendu sans fin : https://github.com/paperclipai/paperclip/issues/3173
- Un commentaire d'agent attribué à l'utilisateur relançait l'agent en boucle : https://github.com/paperclipai/paperclip/issues/3817
- Un agent a écrit un secret en clair dans son journal visible dans l'UI : https://github.com/paperclipai/paperclip/issues/7601
- La clé longue d'un agent lit la config des autres agents : https://github.com/paperclipai/paperclip/issues/18122

## 8. À reprendre pour PneX, à écarter

### 8.1 À reprendre

| # | Quoi | Pourquoi | Où dans PneX |
|---|---|---|---|
| R1 | Budget en argent, avec réservation avant le départ (`held` → `settled` ou `released`) et revérification au premier appel | PneX compte des appels (`llm_calls_max`, compteur quotidien `llm-agent`). Un appel à long contexte coûte cent fois un appel court. La réservation empêche deux runs parallèles de dépasser ensemble. | Passerelle LLM §3.6 (D384) : un `cost_max` par run et par chaîne, un plafond journalier par org en euros, un tarif par fournisseur de l'org. |
| R2 | Usage sans prix = inconnu, donc bloqué par défaut | Un modèle sans tarif connu passerait sous tout plafond en argent. | Même endroit que R1 : refus `agent-gateway-unpriced` si l'org n'a pas de tarif pour ce modèle. |
| R3 | Alerte à 80 % avant l'arrêt | L'humain apprend la dérive avant le refus. Aujourd'hui PneX refuse sans prévenir. | Notifications (nouveau kind), sur le compteur quotidien de l'org et sur la chaîne. |
| R4 | Frein des runs automatiques par org (fermé, ouvert, semi-ouvert), avec sondes et réouverture par paliers | Un flow qui relance `agent_run` sur des échecs brûle le budget de l'org. Le disjoncteur D839 couvre le validateur, pas les runs. Reprendre aussi l'idée du ticket #390 : N runs d'une chaîne sans proposition acceptée ouvrent le frein. | §3.9 Chaînes : état par org, refus `agent-runs-braked` au nœud `agent_run`. Un run lancé depuis la fiche par une personne n'est jamais freiné. |
| R5 | Ordre de la file : run demandé par une personne d'abord, part fixe pour le plus ancien | Un flow bavard ne doit pas faire attendre une personne. | D400 (file à priorités, ⬜) : ordre du claim `FOR UPDATE SKIP LOCKED` du runner (§3.3). |
| R6 | Provenance vérifiée : un run qui déclenche un webhook passe son token de run, et le run créé rejoint la même chaîne | Sans cela, un agent peut sortir des bornes de sa chaîne en appelant un webhook qui lance un flow. | §3.9 : le nœud d'entrée HTTP des flows lit un token de run facultatif et rattache le départ à la chaîne de ce run. |
| R7 | Politique de rattrapage d'un déclencheur planifié : sauter par défaut, sinon rejouer avec un plafond, jamais pour un pas de moins d'une heure | Après un arrêt du backend, éviter une rafale de runs. | Nœud `inject` périodique des flows qui alimente `agent_run`. Vérifier d'abord le comportement actuel de `inject`. |
| R8 | Vérification du harnais avant usage : un run court qui appelle la passerelle une fois, empreinte de l'image vérifiée sur la fiche | Une image qui se construit peut avoir un harnais qui ne parle pas à la passerelle. L'erreur arrive alors au premier vrai run. | §3.4 Environnements : job `probe` sur une version d'environnement, exigé avant de l'épingler dans une fiche (D750). |
| R9 | Règle de confiance tirée d'une décision humaine : même agent, même outil, mêmes arguments (sans `expected_version`), seuil et expiration | Moins de clics sur une action répétée à l'identique. C'est plus sûr qu'un validateur LLM, car rien n'est jugé. | §3.12 Porte : nouveau mode tiré d'une proposition acceptée. Jamais pour la liste D838. Utilité à mesurer d'abord sur les propositions réelles. |
| R10 | Vote sur un résultat de run, avec la raison, versé comme cas dans la Table d'évaluation | La boucle de retour coûte un clic. Elle nourrit D1033 avec de vrais cas. | §3.19 (D1033) : bouton sur la page du run, ligne dans la Table de cas de l'agent. |
| R11 | Taxonomie d'échec des campagnes : produit, modèle, notation, infrastructure | Un score sans classe d'échec pousse à corriger le mauvais côté. | §3.19 (D1033) : une colonne de classe par résultat de campagne. |

### 8.2 Déjà couvert dans PneX

- Validation liée aux arguments exacts : la proposition porte ses `values`, et l'exécution les relit (§3.12).
- Réviseur différent de l'exécutant : D837.
- Quarantaine d'un outil d'écriture nouveau : le mode par défaut d'un outil est `human` (D833).
- Masquage des secrets dans le journal : D733, deux passes. Le ticket #7601 confirme le besoin.
- Coalescence : la clé de sujet de la chaîne et la réponse `joined` (§3.9).
- Révisions en ajout seul : fiches d'agent et compétences versionnées et épinglées (D770–D774).

### 8.3 À écarter

- **Organigramme, agent PDG, `hire_agent`, `approve_ceo_strategy`** : déjà écartés par Shan le 2026-11-24 (§3.19).
- **Réveil par minuterie pour chercher du travail** : il coûte à vide (#3882, #3402). PneX lance un run sur un événement de données, par un flow.
- **Session du harnais reprise d'un run à l'autre** : le contexte grossit sans fin (#3813, #13959). PneX garde l'ontologie comme mémoire entre runs et lance un run neuf.
- **Adaptateurs locaux sans bac** : contraire au bac par run de PneX (§3.5).
- **Compétences permises par défaut à tout agent** : PneX garde l'activation humaine sur `expected_version` (D771).
- **Classifieur LLM d'injection sur les résultats d'outils** : payant, sans garantie selon leur propre doc. La règle structurelle D840 reste la défense.
- **Agent gardien d'arbre de tâches** : un flow sur `agent_run_finished` suffit, sans nouvel objet.
- **Machinerie de reprise** : 80 colonnes sur un run et des dizaines de travaux de reprise produisent des blocages silencieux (#14600, #13532). PneX doit garder peu d'états de run et un bail simple (§3.3).
