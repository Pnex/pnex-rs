# Coffre de secrets par organisation

> **Statut : livré (2026-10-01) — S1 à S8.** S1 :
> module `services::secrets`, trousseau obligatoire au boot, jetons
> internes comparés en temps constant. S2 : migration 000046
> (`org_secrets`, `secret_usages` ; `llm_providers` et le rôle `member`
> viendront avec S7 et S3), API `/api/v1/secrets`, page `/secrets`,
> composant `SecretField`. S4 : canaux de notification en références,
> reprise au boot, snapshot sans valeur, route
> `/internal/flow/secret/{id}` (§11). S5 : nœud http-fetch en références,
> reprise des graphes, éditeur et assistant sans valeur (§12). S6 : WiFi
> en référence, builds sans mot de passe dans la file (§13). S7 :
> fournisseurs LLM en CRUD, défaut plateforme, `PNEX_AI_*` retirés (§14).
> S3 : rôle `member`, revue des routes d'écriture (§15). S8 : rotation
> de la clé maîtresse et exploitation (§16). Remplace
> et élargit N5
> (`notifications.md` §6, D54). Décisions D110–D119.
>
> **Première release (2026-10-01, D120, `migrations.md`)** : les reprises
> au boot (S4–S7), la table `ai_connectors` et la colonne
> `wifi_credentials.wifi_password` sont retirées — aucune base antérieure
> au coffre n'est à reprendre. Les numéros de migration cités plus bas
> renvoient à l'historique pré-release, fondu dans la migration de base.

## 1. Constat (inventaire du 2026-10-01)

Aucun chiffrement au repos dans le dépôt : pas de crate AEAD, seul
`chacha20` (trames device D8, sans authentification) est présent.

| Secret | Stockage aujourd'hui | Fuite constatée |
|---|---|---|
| Canaux de notification (token ntfy, bot telegram, URL slack/discord, mot de passe smtp, secret webhook) | `notify_channels.config` JSONB en clair | Write-only à l'API, mais **copiés en clair dans `flows.json`** au deploy (snapshot D50) et dans `ApplyOrg` entre pods (D106) |
| Nœud http-fetch (basic, bearer, en-tête, proxy) | `flow_versions.graph` en clair | **Visible des viewers** via `GET /flows/{id}` ; en clair dans `flows.json` et `ApplyOrg` |
| WiFi (SSID + PSK) | `wifi_credentials.wifi_password` en clair | **Renvoyé en clair à tous les membres**, viewers compris ; persisté dans `pg_loco_queue.task_data` à chaque build |
| Clé fournisseur LLM | `ai_connectors.api_key` en clair (1 par org) + `PNEX_AI_*` en env | Write-only à l'API (correct) |
| Clés device, token O2 par org | En clair | Hors périmètre v1 (§9) |

Le commentaire `flow_supervisor/mod.rs` « jamais de secret dans
flows.json » est faux aujourd'hui.

## 2. Objectifs

1. **Un seul endroit** où tous les secrets d'une org existent, chiffrés.
2. **Une page CRUD centrale** qui les liste tous (sans jamais montrer de
   valeur), avec « utilisé par ».
3. **Édition au lieu fonctionnel** : le formulaire d'un canal de
   notification, d'un nœud HTTP, d'un WiFi ou d'un fournisseur LLM écrit
   dans le même coffre.
4. **Pas de templating** : un champ secret contient une **référence typée**
   vers une ligne du coffre, jamais une chaîne `{{…}}`.
5. **Rôles** : l'admin configure, le membre réutilise sans voir.
6. **Plus aucun secret en clair** en base, dans `flows.json`, dans la file
   de jobs ou sur le fil inter-pods.
7. **Plus de configuration LLM en variables d'environnement** : CRUD de
   fournisseurs, avec un défaut.

## 3. Décisions

| # | Décision |
|---|---|
| **D110** | **Coffre unique `org_secrets`.** Une ligne = **une valeur secrète** (une chaîne). Les parties non secrètes restent dans la config fonctionnelle : SSID dans `wifi_credentials`, nom d'en-tête et utilisateur basic dans le nœud, URL de base dans le fournisseur LLM. Colonnes : `id`, `org_id` (NULL = plateforme, D116), `name` (unique par org), `description`, `ciphertext`, `nonce`, `key_id`, `created_by`, `updated_by`, `created_at`, `updated_at`. |
| **D111** | **Pas de portée ni de catégorie** (flow, app, WiFi… : un secret est un secret, réutilisable partout). La frontière de sécurité est l'**usage** : le runtime de flows ne peut résoudre un secret que s'il est référencé par un flow **déployé** de l'org placée sur ce worker (`secret_usages`, D114). Une clé LLM qu'aucun flow ne référence reste donc illisible par le runtime. |
| **D112** | **Chiffrement XChaCha20-Poly1305** (crate `chacha20poly1305`). Nonce aléatoire 24 octets par écriture. **AAD = `org_id ‖ secret_id`** : une ligne recopiée sur une autre org ou un autre secret ne se déchiffre pas. Clé maîtresse hors base, en env : **trousseau** `PNEX_SECRETS_KEYS="k2:<b64>,k1:<b64>"`, la première est la clé d'écriture, les suivantes servent seulement à relire. `key_id` sur chaque ligne. **Absente = refus de démarrer** (école D108) ; le yaml de dev porte une clé de dev fixe. |
| **D113** | **Références typées, pas de templating.** Un champ secret d'une config est `SecretRef { id: Uuid }`, sérialisé `{"secret_id": "…"}`. Côté UI, un composant unique `SecretField` propose deux modes : **saisir une valeur** (crée ou remplace un secret dédié, nommé d'après son lieu, ex. `notify/alertes-astreinte/token`) ou **choisir un secret existant** (sélecteur de tous les secrets de l'org). |
| **D114** | **Traçage des usages `secret_usages`** (`secret_id`, `consumer_kind`, `consumer_id`, `field`), réécrit à chaque sauvegarde d'un consommateur (canal, version de flow, WiFi, fournisseur LLM). Sert la colonne « utilisé par » du CRUD central et le **refus de suppression d'un secret utilisé** (409 `secret-in-use`). Un secret dédié est supprimé avec son consommateur quand il n'a plus d'autre usage. |
| **D115** | **Le runtime ne reçoit jamais de valeur dans `flows.json` ni dans `ApplyOrg`** : uniquement des `secret_id`. Il les résout à la construction du nœud via `GET /internal/flow/secret/{id}` (même modèle que les modèles ML de `pnex-node-vision`). Contrôles : jeton runtime (comparaison en temps constant), **org du secret = org placée sur ce worker** (fencing D106), secret **référencé par une version déployée** d'un flow de cette org (D111). Valeur gardée **en mémoire seulement**, jamais loguée. La clé maîtresse **ne va jamais** dans le runtime : il exécute du code utilisateur multi-org (JS/Starlark). Les nœuds fonction **n'ont pas accès** aux secrets. |
| **D116** | **Fournisseurs LLM en CRUD.** Table `llm_providers` (org_id NULL = plateforme) : nom, type (`anthropic` / `openai-compat`), URL de base, modèle, `SecretRef`, `is_default`. Résolution : défaut de l'org, sinon défaut plateforme (configuré par l'admin plateforme D72 dans /system), **visible et utilisable par toutes les orgs** (sa clé reste illisible pour elles). **Retrait sec des `PNEX_AI_*`**, sans import : un fournisseur LLM n'est pas nécessaire au démarrage, c'est à l'utilisateur de le provisionner dans l'UI. Sans fournisseur, l'assistant IA s'affiche comme non configuré. Remplace `ai_connectors`. |
| **D117** | **Rôles : ajout de `member`** entre admin et viewer. owner/admin : CRUD du coffre, saisie et remplacement de valeur. member : édite flows et canaux, **choisit** un secret existant, ne peut ni créer, ni modifier, ni supprimer un secret. viewer : lecture seule, voit les noms des secrets référencés, rien d'autre. Aucun rôle ne relit une valeur (pas de « révéler » en v1). |
| **D118** | **Changer une valeur ne déclenche rien en cascade.** Flows : la nouvelle valeur est prise au **prochain deploy** de l'org (pas de rechargement à chaud). WiFi : **aucune recompilation automatique** des firmwares qui l'embarquent ; les devices passent hors ligne et l'utilisateur rebranche et reflashe device par device avec le nouvel identifiant. Recompiler tout un parc d'un coup chargerait le serveur pour rien, alors qu'un par un c'est plus doux. L'UI le signale à la sauvegarde (« les devices déjà flashés gardent l'ancien identifiant »). |
| **D119** | **Pas de LLM plateforme (2026-10-01, remplace la partie plateforme de D116).** L'utilisateur apporte son propre LLM : les fournisseurs n'existent qu'au niveau de l'org, gérés à **un seul endroit** (détail de l'organisation, `/orgs/current`). Résolution = défaut de l'org, **sans repli plateforme** ; routes `/api/v1/system/ai/providers` supprimées. La migration 000050 supprime les lignes `org_id IS NULL`, leurs usages et leurs secrets dédiés (`llm/<nom>/api_key` plateforme). |

## 4. Modèle de données (migration 000046)

- `org_secrets` (D110), unique `(org_id, name)` ; un index partiel gère
  les secrets plateforme (`org_id IS NULL`).
- `secret_usages` (D114), unique `(consumer_kind, consumer_id, field)`.
- `llm_providers` (D116), un seul `is_default` par org (index partiel).
- Valeur `member` ajoutée à l'enum `org_member_role`.
- **La reprise des données n'est pas une migration SQL** : elle exige la
  clé. C'est une tâche Rust au boot, idempotente, sous verrou consultatif
  (école des migrations multi-pods).

## 5. API

- `GET /api/v1/secrets` : nom, description, `updated_at/by`,
  usages. **Jamais de valeur.** Pagination et recherche SQL. Tous les
  rôles, mais un viewer ne voit que les noms.
- `POST /api/v1/secrets`, `PUT /api/v1/secrets/{id}` (nom, description,
  nouvelle valeur optionnelle), `DELETE` (409 si utilisé) : owner/admin.
- Formulaires fonctionnels : le champ secret accepte
  `{"secret_id": …}` (choix) ou `{"value": "…"}` (saisie, réservée
  owner/admin). En lecture, il renvoie `{"secret_id", "name"}`.
- Nouveaux codes d'erreur : `secret-not-found`, `secret-in-use`,
  `secret-not-referenced`, `secret-name-taken`, `secret-write-forbidden`,
  avec leurs clés fluent fr/en.

## 6. UI

- **Page `/secrets`** dans les réglages de l'org (socle CRUD) : table
  nom / utilisé par (liens vers le canal, le flow, le WiFi, le
  fournisseur) / modifié le / par. Actions : créer, renommer, remplacer la
  valeur, supprimer.
- **`SecretField`** réutilisé partout : canal de notification, nœud
  http-fetch (auth et proxy), WiFi, fournisseur LLM. Affiche « défini ·
  nom du secret » et les actions selon le rôle.
- **Page fournisseurs LLM** (org) + bloc « fournisseur par défaut » dans
  /system (plateforme).

## 7. Rotation

- **Valeur d'un secret** (D118) : remplacée dans le coffre. Flows : effet
  au prochain deploy de l'org. Firmwares : aucun rebuild automatique, les
  devices concernés sont reflashés un par un par l'utilisateur.
- **Clé maîtresse** : ajouter `k3` en tête du trousseau, redémarrer, puis
  lancer la tâche de rechiffrement (singleton `task:secrets-rekey`) qui
  réécrit les lignes `key_id != primaire`. Retirer l'ancienne clé quand le
  compteur affiché dans /system est à zéro.

## 8. Lots

| Lot | Contenu | Utile seul ? |
|---|---|---|
| **S1** | Crypto + trousseau (module `secrets` côté backend, tests vecteurs + AAD), clé obligatoire au boot | Non (fondation) |
| **S2** | Tables, API `/secrets`, `secret_usages`, codes d'erreur, page `/secrets`, `SecretField` | Oui : coffre manuel |
| **S3** | Rôle `member` (D117) : enum + revue de toutes les routes d'écriture existantes (`can_write` → owner/admin/member selon la ressource, coffre et membres réservés owner/admin) + UI de gestion des membres | Oui |
| **S4** | Canaux de notification → références ; reprise des secrets existants ; snapshot sans valeur | Oui : ferme N5 |
| **S5** | http-fetch → références ; route `/internal/flow/secret/{id}` ; `flows.json` et `ApplyOrg` sans valeur ; reprise des graphes de flow | Oui : ferme les fuites viewer et disque |
| **S6** | WiFi → référence ; les builds transportent l'id, le worker déchiffre à l'exécution (comme le token device) | Oui |
| **S7** | Fournisseurs LLM en CRUD + défaut plateforme visible de toutes les orgs + retrait sec de `PNEX_AI_*` (yaml, compose, doc) | Oui |
| **S8** | Rechiffrement de la clé maîtresse + doc d'exploitation | Oui |

Ordre conseillé : S1 → S2 → S4 → S5 → S6 → S7 → S3 → S8. S3 peut
remonter si le rôle membre est attendu vite.

E2E de sortie : un admin crée un canal telegram et un nœud HTTP bearer ;
un membre construit un flow en choisissant ces secrets sans les voir ;
deploy ; `flows.json` et la base ne contiennent aucune valeur en clair
(grep) ; l'envoi et l'appel HTTP fonctionnent ; remplacement de la valeur
puis redeploy pris en compte ; suppression refusée tant qu'elle est
utilisée.

## 9. Hors périmètre v1 (tickets séparés)

- **Secrets d'infrastructure** (`DATABASE_URL`, clés S3, jetons cluster,
  runtime et notify, trousseau du coffre) : restent en env/SOPS. Le coffre
  ne peut pas contenir sa propre clé.
- **Clés device et tokens agent** (`device_tokens`) : identifiants
  machine générés, pas des secrets gérés par l'utilisateur. Le
  chiffrement au repos est possible plus tard avec le même module.
- **Mot de passe root O2 injecté dans le runtime** : à remplacer par le
  token d'ingestion par org (fuite multi-org indépendante du coffre).
- **Comparaisons de jetons en temps constant** sur toutes les routes
  internes : correctif immédiat, à faire dès S1.
- **Révéler une valeur**, coffre externe (Vault, KMS), clé par org
  (crypto-shredding) : v2 si besoin.

## 10. Tranchages (2026-10-01)

1. Rôle `member` : **ajouté** (D117).
2. Fournisseur LLM par défaut de la plateforme : ~~visible par toutes les
   orgs (D116)~~ → **supprimé** : chaque org apporte son LLM (D119).
3. `PNEX_AI_*` : **retrait sec**, pas d'import (D116).
4. Rotation : **redeploy** pour les flows, **rien d'automatique** pour le
   WiFi (D118).

## 11. Notes de livraison S4 (2026-10-01)

- **Stockage** : un champ `secret` d'un canal vaut `{"secret_id": …}` en
  base. À l'API il accepte `{"secret_id"}` (choix), `{"value"}` ou une
  chaîne nue (saisie, secret dédié `notify/<canal>/<champ>`), `null` ou
  absent (inchangé), `""` (effacé). En lecture : `secrets` =
  `{champ: {secret_id, name}}`, en plus de `secrets_set`.
- **Validation** sur la valeur résolue (choix déchiffré côté serveur,
  jamais renvoyé) : les règles des canaux (URL Slack/Discord, token
  Telegram) restent appliquées.
- **Secret dédié** : renommé avec le canal, supprimé quand le canal
  choisit un autre secret ou est supprimé (s'il n'a plus d'usage).
- **Reprise** : tâche Rust au boot (`secrets::takeover_at_boot`, avant la
  projection des flows), idempotente, sous verrou consultatif global
  `SECRETS_TAKEOVER`. Non bloquante : un échec est logué, le canal garde
  son clair jusqu'au boot suivant (le nœud tolère une chaîne).
- **Runtime** : le snapshot ne porte que des références. Le nœud
  `pnex-notify` résout **au premier envoi** (et non à la construction
  comme écrit en D115) puis garde la valeur en mémoire jusqu'au prochain
  deploy. Raison : le deploy applique le fragment au runtime **avant** de
  marquer le flow déployé en base ; un contrôle « référencé par une
  version déployée » à la construction refuserait le premier deploy. Le
  nœud réessaie brièvement un 404 (fenêtre entre l'acquittement et le
  marquage).
- **Contrôle de `/internal/flow/secret/{id}?org_id=`** : jeton runtime
  (`x-pnex-flow-token`, temps constant), fencing D106, secret de l'org,
  tenu par un canal **actif** de l'org qu'un nœud notify d'un flow
  **déployé** de l'org utilise. Introuvable et non référencé répondent le
  même 404 (`secret-not-referenced`). Désactiver un canal coupe donc ses
  envois dès le prochain premier envoi, sans redeploy. S5 étendra le
  contrôle aux usages `flow` (http-fetch).
- **Tests d'envoi** : `test` et `test-draft` résolvent en mémoire ;
  `test-draft` accepte `channel_id` pour compléter les champs inchangés
  d'un canal en cours d'édition. Rien n'est écrit dans le coffre par un
  test.

## 12. Notes de livraison S5 (2026-10-01)

- **Modèle** : `pnex_core::SecretSlot` (`Unset` | `Ref(id)` | `Value`)
  remplace les quatre champs secrets du nœud http-fetch : mot de passe
  basic (`auth.password`), token bearer (`auth.token`), valeur de l'en-tête
  de clé (`auth.value`), mot de passe proxy (`proxy.password`). Lecture
  tolérante : `null`/`""` → `Unset`, chaîne ou `{"value"}` → `Value`,
  `{"secret_id"}` (avec ou sans `name`) → `Ref`. Les en-têtes statiques,
  l'utilisateur basic/proxy et le nom de l'en-tête restent en clair (D110).
- **Sauvegarde** (`create_flow` / `append_version`, dans la transaction) :
  chaque `Value` devient le secret dédié `flow/<flow_id>/<node_id>/<champ>`
  (créé ou remplacé sur place), chaque `Ref` doit appartenir à l'org (404
  `secret-not-found` sinon). La réponse renvoie le graphe **stocké** ;
  l'éditeur y adopte les références (`adopt_stored_secrets`) pour ne plus
  garder la valeur saisie ni se croire modifié.
- **Usages** : consommateur `flow` = références de la dernière version **et**
  de la version déployée (préfixe `deployed/` si elles diffèrent), réécrits
  à chaque sauvegarde et à chaque deploy. Un secret utilisé par la version
  qui tourne reste donc insupprimable (409).
- **Secrets dédiés** : supprimés avec le flow. Ils ne sont pas élagués à la
  sauvegarde, pour qu'un rollback vers une ancienne version retrouve ses
  secrets ; un nœud retiré laisse un secret « non utilisé », supprimable à
  la main depuis `/secrets`.
- **Assistant IA** : ne saisit jamais de valeur (`WriteForbidden`), il peut
  seulement garder ou choisir une référence. Il ne voit plus de valeur dans
  `get_flow` (le graphe ne contient que des références).
- **Reprise** : `flow::takeover` au boot, sous le même verrou que S4,
  versions traitées de la plus ancienne à la plus récente. Un même champ de
  nœud partagé entre versions pointe vers un seul secret dédié qui garde la
  dernière valeur : un rollback tourne avec la valeur courante (D118).
- **Runtime** : comme S4, résolution au premier appel puis cache mémoire
  jusqu'au prochain deploy. Le client HTTP (proxy authentifié compris) est
  construit à ce moment-là ; une URL de proxy invalide reste refusée dès la
  construction. Le contrôle de `/internal/flow/secret/{id}` accepte aussi
  un secret référencé directement par un champ d'un graphe déployé de
  l'org.

## 13. Notes de livraison S6 (2026-10-01)

- **Modèle** : migration 000047, `wifi_credentials.secret_id` (uuid, sans
  clé étrangère : SQLite ne sait pas l'ajouter par `ALTER TABLE` ;
  l'intégrité passe par `secret_usages`). La colonne `wifi_password` reste,
  vidée par la reprise et plus jamais écrite (suppression d'une colonne
  fragile sous SQLite, et lue par d'anciens pods pendant une montée de
  version progressive).
- **API** : `WifiCredential` perd `wifi_password` et gagne
  `password: {secret_id, name}`. `WifiCredentialInput.password` =
  `{"secret_id"}` ou `{"value"}` (secret dédié `wifi/<ssid>/password`,
  owner/admin) ; absent à l'édition = inchangé ; l'ancien `wifi_password`
  en clair reste accepté comme saisie. Le secret dédié suit le renommage
  du SSID et disparaît quand l'entrée choisit un autre secret ou est
  supprimée.
- **Builds** : `CreateBuild.wifi_credential_id` (le wizard et le rebuild
  l'envoient). Le job ne porte plus que `wifi_secret_id` ; le worker
  déchiffre au moment du build, comme il relit déjà le token du device.
  Une requête héritée (SSID + mot de passe) crée ou met à jour l'entrée du
  référentiel avant le build : la valeur ne transite jamais par
  `pg_loco_queue`. Un job mis en file par une ancienne version (champ
  `wifi_password`) reste lisible.
- **Rotation (D118)** : l'édition d'une entrée affiche que les devices
  déjà flashés gardent l'ancien identifiant ; aucun rebuild automatique.
- **Reprise** : `wifi::takeover` au boot, sous le verrou commun.

## 14. Notes de livraison S7 (2026-10-01)

- **Modèle** : migration 000048, `llm_providers` (`org_id` NULL =
  plateforme, `secret_id` sans clé étrangère comme pour le WiFi, un seul
  `is_default` par propriétaire par index partiels, noms uniques par
  propriétaire). Le premier fournisseur créé devient le défaut.
- **API** : `/api/v1/ai/providers` (org : lecture pour tous les membres,
  écriture owner/admin, code `llm-provider-forbidden`) et
  `/api/v1/system/ai/providers` (admin plateforme). Clé = `SecretFieldInput`
  (secret dédié `llm/<nom>/api_key`, du propriétaire du fournisseur), absente
  à l'édition = inchangée, obligatoire à la création. Le secret dédié suit
  le renommage et disparaît avec le fournisseur. `POST …/{id}/test` = un
  ping réel. Les routes `/ai/connector*` et `/system/ai/test` sont retirées.
- **Résolution** : défaut de l'org, sinon défaut plateforme, sinon non
  configuré ; `/ai/status` expose `source` (`org` | `platform`) et
  `provider_name`. Une org voit le défaut plateforme dans sa liste
  (`platform: true`) sans référence de clé.
- **Environnement** : `PNEX_AI_PROVIDER`, `PNEX_AI_BASE_URL`,
  `PNEX_AI_API_KEY`, `PNEX_AI_MODEL` retirés des yaml sans import. Seul le
  kill-switch `PNEX_AI_ENABLED` reste : c'est un interrupteur, pas une
  configuration de fournisseur. Aucun compose ni pnex-deploy ne les posait.
- **Reprise** : `llm::takeover` au boot, sous le verrou commun : chaque
  `ai_connectors` devient un fournisseur de l'org (nommé d'après son type,
  par défaut si l'org n'en a pas), sa clé passe au coffre, la ligne est
  supprimée. La table `ai_connectors` sera supprimée par une migration
  ultérieure, une fois les instances passées par ce boot.
- **UI** : composant `LlmProviders` (détail de l'org et /system) ;
  `SecretField` gagne `typed_only` (côté plateforme, pas de choix parmi
  les secrets de l'org courante).

## 15. Notes de livraison S3 (2026-10-01)

- **Modèle** : migration 000049, valeur `member` ajoutée à l'enum
  PostgreSQL `org_member_role` entre `admin` et `viewer` (SQLite stocke
  l'enum en texte : rien à faire ; `down` vide, PostgreSQL ne retire pas
  une valeur d'enum).
- **Deux niveaux côté backend** : `OrgContext::can_write()` (owner, admin,
  member) garde **tout le contenu** : flows et leur deploy, devices et
  commandes de pins, agents edge, dashboards, widgets, POI, tours,
  annotations, médias, caméras, modèles ML, fonctions, firmwares et builds,
  référentiels edge (WiFi, hôtes), canaux et modèles de notification,
  mélanges, ressources (labels, dossiers, arêtes), assistant IA en
  écriture. `OrgContext::can_administer()` (owner, admin) garde la
  **gouvernance** : renommer l'org, gérer ses membres, fournisseurs LLM de
  l'org, suppression de données de télémétrie (/system). Le coffre garde
  son propre contrôle `can_manage_secrets()` (owner, admin) : un membre
  **choisit** un secret existant dans un champ secret, toute saisie de
  valeur est refusée (403 `secret-write-forbidden`). Supprimer l'org et
  gérer les owners restent réservés aux owners.
- **Agents edge** : ouverts aux membres. Un agent est un device, et un
  membre crée déjà des devices et lance des builds porteurs de jetons ; le
  réserver aux admins n'aurait rien protégé.
- **Messages** : les codes d'erreur ne changent pas ; leurs descriptions
  anglaises et les clés fluent annoncent « owner, admin ou membre » pour
  les routes de contenu.
- **UI** : helpers `role_can_write` / `role_can_administer`
  (`state/org.rs`) en miroir du backend, utilisés par toutes les pages ;
  rôle « Membre » dans les sélecteurs d'ajout et de modification, badge
  vert, légende des quatre rôles sous la liste des membres. Le rôle par
  défaut d'un ajout reste observateur (moindre privilège).
- **Test** : `members_pick_secrets_but_never_administer` (attribution du
  rôle par l'API, saisie refusée, choix accepté sans valeur exposée,
  gouvernance refusée).

## 16. Notes de livraison S8 et exploitation (2026-10-01)

### Livré

- **`services::secrets::rekey`** : `stats` (lignes totales, lignes hors clé
  d'écriture, lignes sous une clé absente du trousseau) et `rekey`
  (parcours par id, par lots de 200). Chaque ligne est déchiffrée puis
  rechiffrée avec la clé d'écriture par un `UPDATE` conditionné sur son
  ancien nonce et son ancien `key_id` : un remplacement de valeur
  concurrent (nouveau nonce) n'est jamais écrasé par l'ancienne valeur. La
  valeur, `updated_at` et `updated_by` ne changent pas. Une ligne
  illisible (clé absente, échec d'authentification) est laissée telle
  quelle, son id est journalisé (jamais la valeur).
- **Singleton** : `rekey_singleton` sous le verrou consultatif
  `SECRETS_REKEY` (global), comme la reprise du boot. Un second
  lancement attend le premier, puis ne trouve plus rien à faire.
- **Déclencheurs** : bouton « Rechiffrer les secrets » dans
  /admin/status (admin plateforme, `POST /api/v1/system/secrets/rekey`,
  qui renvoie `rewritten`, `unreadable`, `skipped`, `remaining`) et tâche
  `secrets_rekey` pour l'exploitation sans UI. **Jamais au boot** : en
  rolling upgrade, un pod qui rechiffre avant que les autres connaissent
  la nouvelle clé rendrait le coffre illisible pour eux.
- **Supervision** : composant « Coffre de secrets » dans /admin/status
  (clé d'écriture, nombre de clés, secrets stockés, secrets sous une
  ancienne clé). État dégradé si des lignes sont sous une clé absente du
  trousseau.
- **Test** : `rekey_moves_every_readable_row_to_the_write_key` (org et
  plateforme rechiffrés et relisibles avec la seule nouvelle clé, ligne
  orpheline intacte, idempotence, route réservée à l'admin plateforme).

### Exploitation

- **Format** : `PNEX_SECRETS_KEYS="<id>:<base64 de 32 octets>,…"`. La
  première entrée chiffre, les suivantes ne servent qu'à relire. Un id
  fait au plus 32 caractères `[A-Za-z0-9_-]`. Générer une clé :
  `openssl rand -base64 32`. pnex-deploy la génère à l'installation.
- **Sauvegarde** : la clé est **aussi critique que la base**. Une
  sauvegarde de la base sans le trousseau ne restitue aucun secret
  (canaux, nœuds HTTP, WiFi, fournisseurs LLM). Ranger le trousseau à
  part de la base (gestionnaire de mots de passe, coffre d'infra), jamais
  dans le même dump.
- **Rotation** (fuite suspectée, départ d'un opérateur, hygiène) :
  1. Générer `k2`, poser `PNEX_SECRETS_KEYS="k2:<new>,k1:<old>"`.
  2. Redémarrer **tous** les pods (API et workers de build). Vérifier
     dans /admin/status que la clé d'écriture affichée est `k2`.
  3. Lancer le rechiffrement (bouton, ou `pnex-server task
     secrets_rekey`). Le compteur « secrets sous une ancienne clé » doit
     tomber à zéro.
  4. Retirer `k1` du trousseau et redémarrer.
  Une rotation de clé ne change aucune valeur : si la valeur elle-même a
  fuité, la remplacer dans /secrets (D118).
- **Clé perdue** : les lignes concernées sont définitivement illisibles.
  /admin/status passe le coffre en dégradé ; les consommateurs échouent
  proprement (canal en erreur, nœud HTTP refusé à la construction, build
  WiFi en échec). Remède : ressaisir chaque valeur depuis l'UI (la
  saisie rechiffre avec la clé d'écriture).
- **Hors périmètre** : le runtime de flows ne reçoit jamais la clé (D115),
  il n'y a rien à faire de son côté.
