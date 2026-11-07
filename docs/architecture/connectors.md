# Connecteurs : cadre commun, bases, REST générique, webhooks entrants (D791–D797)

| | |
|---|---|
| Statut | **Proposé le 2026-11-07, choix produit tranchés par Shan le même jour** (§3). Rien de codé au-delà du premier lot PostgreSQL du 2026-11-05 |
| Déclencheur | Ordre OP1 → OP2 → OP3 (`operational-platform.md` §2) ; OP1 livré en grande partie |
| Repose sur | `operational-platform.md` (D555), `data-pipelines.md` (D352, §9.6 « Connecteurs, premier lot »), `secrets.md` (D113, D115), `security.md` (R1, R2, R4, R8, R16), `iam.md`, `crm.md` (D369) |
| Numérotation | **D791…D797**, lots **CN1…CN5** |
| Revient sur | D352 (`elasticsearch` et `http` sortent de la liste des sortes, `rest` et `webhook` y entrent) ; la ligne « stockage cloud » d'OP2 (D796) |

## 1. Le besoin en une phrase

Faire entrer dans une table versionnée les données d'une base, d'une API REST ou d'un système qui
pousse, avec les mêmes garde-fous (coffre, egress, droits, lignage) que le connecteur PostgreSQL
déjà livré, sans écrire de code par éditeur.

## 2. Constat (relevé dans le code, 2026-11-07)

| Point | État |
|---|---|
| Ressource | `connectors` (migration 000078), `services/connectors.rs`, routes `/api/v1/connectors`, droits `connectors:read\|write\|use\|test\|delete` |
| Sortes | PostgreSQL seul (`ConnectorKind::Postgres`) |
| Exécution | Étape `connector_query` d'un import (`pnex_core::ingest::IngestStep`) : résultat en CSV dans le staging, puis chemin de tout fichier (table, synchro, planification, événements) |
| Réseau | `egress::check_host` + `filter_addrs`, liste d'hôtes privés par org posée par l'admin plateforme, connexion vers l'adresse vérifiée |
| Secret | Mot de passe au coffre ; connecteur déplacé = mot de passe redemandé ; tout le connecteur `EXCLUDED` du MCP |
| Écran | Onglet Connecteurs de `/pipelines`, sans CRUD complet |
| Entrant | Aucun point d'arrivée pour un système qui pousse |

## 3. Choix de Shan (2026-11-07)

| Question | Réponse |
|---|---|
| Premier système métier | Connecteur REST générique déclaratif, pas de connecteur par éditeur |
| MySQL | TLS obligatoire, exception `deny.toml` motivée par ça |
| CDC | Incrémental par colonne curseur ; pas de réplication logique |
| Écriture retour | Attend OP9 (D563) : OP2 est en lecture seule |
| Stockage cloud | Aucun stockage externe : seulement le S3 déjà en place pour PneX, « sinon ça fait trop de config à gérer » |
| Tables virtuelles | Plus tard, après OP3 |
| Webhooks entrants | Vers une table versionnée |
| Écran | Oui, dans l'onglet Connecteurs existant |

## 4. Décisions

### D791 — Un cadre, quatre sortes, lecture seule

Un connecteur garde sa forme actuelle (nom, sorte, cible, identifiants au coffre) et une sorte parmi
`postgres`, `mysql`, `rest`, `webhook`. Tout ce que le premier lot a posé vaut pour chaque sorte :
coffre, egress vers l'adresse vérifiée, droits `connectors:*`, test sans retour de données,
exclusion du MCP, secret redemandé si la cible change. En OP2, aucune sorte n'écrit chez le tiers
(D555 : l'écriture retour est une action de l'ontologie soumise à D563, lot OP9).

### D792 — MySQL sous TLS seulement

`sqlx-mysql` tire `rsa` pour chiffrer le mot de passe `caching_sha2_password` quand la connexion
n'est pas chiffrée. Sous TLS, ce chemin n'est jamais emprunté. Un connecteur `mysql` exige donc TLS,
y compris vers un hôte listé ; l'exception `RUSTSEC-2023-0071` de `deny.toml` est étendue avec ce
motif. Même garde de lecture seule que PostgreSQL : `SET SESSION TRANSACTION READ ONLY`, une seule
instruction (préparée d'abord), délai et plafond d'octets.

### D793 — REST générique déclaratif

Le connecteur porte l'URL de base et l'authentification ; l'import porte la requête.

- **Authentification** : `none`, `bearer`, `header` (nom d'en-tête + valeur), `basic`,
  `oauth2_client_credentials` (URL de jeton, client, secret, portée ; jeton gardé en mémoire jusqu'à
  expiration). Toute valeur sensible au coffre. Pas de flux OAuth2 avec consentement en OP2.
- **Requête** (étape `connector_query`, champ `rest`) : `path`, `params`, `records` (pointeur JSON
  vers le tableau d'enregistrements), `columns` (nom de colonne → pointeur JSON dans
  l'enregistrement ; sans `columns`, les clés de premier niveau du premier enregistrement).
  `GET` seulement.
- **Pagination** : `none`, `page` (paramètre de page, taille), `offset` (paramètre, taille),
  `cursor` (pointeur JSON du curseur suivant, paramètre où le renvoyer), `link` (en-tête `Link`
  `rel="next"`). Arrêt sur page vide, curseur absent ou plafond.
- **Garde-fous** : chaque URL de page passe l'egress ; une URL suivante doit garder l'hôte de
  l'URL de base, sinon refus ; redirections non suivies ; plafond d'octets (même défaut que
  PostgreSQL), de pages (10 000), de durée (30 min) ; `429` et `5xx` relancés avec attente
  (`Retry-After` respecté, 5 essais) ; réponse non JSON = erreur codée.
- **Sortie** : CSV dans le staging, comme `connector_query` PostgreSQL. Une valeur non scalaire
  part en texte JSON.

Un SaaS ou un ERP (Odoo, CRM, facturation) se branche par un connecteur `rest` décrit en YAML ;
une règle par défaut peut vivre dans un pack, jamais un site en dur dans le moteur.

### D794 — Incrémental par colonne curseur

Une requête déclare `cursor` : colonne SQL ou champ de l'enregistrement REST, et pour REST le
paramètre qui reçoit la dernière valeur. La dernière valeur vue est gardée sur l'import et
**avancée seulement après le commit de la table** : un run qui casse relit la même tranche (au
moins une fois), la synchro dédoublonne par sa clé. Côté SQL, la requête lit `:cursor`, lié par le
pilote, jamais interpolé. Les suppressions à la source ne se voient pas : `full_every` (en runs)
relance une lecture complète, qui ferme les objets absents comme tout import complet.

### D795 — Webhooks entrants vers une table

Un connecteur `webhook` reçoit une URL `POST /api/v1/hooks/{id}` (route `Public` au registre IAM :
c'est la signature qui authentifie).

- **Signature** : HMAC-SHA256 du corps avec le secret du connecteur (coffre), en-tête et préfixe
  configurables (`X-Signature`, `sha256=`), comparaison en temps constant. En-tête d'horodatage
  optionnel : hors de ±5 min = refus (rejeu). Identifiant inconnu, signature absente ou fausse :
  même `401`, sans dire lequel.
- **Org** : celle du connecteur, jamais une valeur de la requête (R1).
- **Réception** : corps JSON de 1 Mio au plus, débit plafonné par connecteur ; le corps entre dans
  une boîte de réception PostgreSQL et la réponse est `202` tout de suite.
- **Vers la table** : l'ordonnanceur d'OP1.4 vide la boîte par lots (une transaction de table
  par lot, jamais une par appel) selon les `records` et `columns` de D793, puis la synchro
  ontologie suit comme pour un import. Boîte pleine (plafond par org) = `503` codé, l'émetteur
  relance.

### D796 — Pas de stockage cloud externe

La ligne « stockage cloud » d'OP2 se réduit au S3 déjà configuré pour PneX. Un fichier entre par
l'envoi en plusieurs parties (`POST /datasets/{id}/uploads`), `pnex dataset load` ou un import
par URL. Aucun identifiant de compartiment tiers à gérer.

### D797 — Hors OP2

Écriture retour (OP9), tables virtuelles (après OP3, sinon elles contournent les droits par
ligne), réplication logique PostgreSQL, Elasticsearch (repris par `rest` si l'index expose
`_search` en `GET`), SFTP, connecteurs nommés par éditeur, OAuth2 avec consentement.

## 5. Sécurité (grille `security.md` §6)

- **R1** : org du principal pour les routes, du connecteur pour le webhook.
- **R2** : `connectors:use` revérifié sur l'auteur de l'import à chaque run (déjà en place) ;
  test sans retour de données ; webhook écrit avec les droits de l'auteur du connecteur.
- **R4 / R16** : aucun jeton, mot de passe ni secret HMAC dans une réponse, un journal d'étape ou
  une erreur ; l'URL du webhook n'est pas un secret (la signature l'est).
- **R8** : chaque page REST, chaque appel de jeton OAuth2 passe l'egress ; hôte constant sur la
  pagination ; pas de redirection.
- **Entrée hostile** : JSON profondément imbriqué plafonné (profondeur 64), corps et réponses
  plafonnés avant parsing.
- **MCP** : connecteurs toujours `EXCLUDED` (secret) ; la route webhook `EXCLUDED` (entrant
  machine).

## 6. Assistant, i18n, contrat

- Nouveaux codes d'erreur (`connector-tls-required`, `connector-rest-not-json`,
  `connector-rest-host-changed`, `connector-rest-page-limit`, `hook-inbox-full`…) dans
  `err_codes::ALL` et les deux `.ftl`.
- Fiche `assistant-kb` « connecteurs » : sortes, incrémental, webhook, pannes courantes
  (signature refusée, hôte non listé) avec le geste dans l'UI.
- Routes annotées, `task openapi`, `exposure.rs`, `iam.rs` dans le même commit que chaque route.

## 7. Lots

| Lot | Contenu | Livre | Taille |
|---|---|---|---|
| **CN1** | D793 : sorte `rest`, auth, pagination, colonnes | Un import depuis une API paginée de test (serveur local) jusqu'à la table | M |
| **CN2** | D794 : curseur SQL et REST, `full_every` | Deux runs : le second ne lit que le neuf ; un run cassé relit sa tranche | S |
| **CN3** | D795 : webhook, boîte, vidage par lots | 1 000 appels signés → une table, appels mal signés refusés | M |
| **CN4** | D792 : MySQL sous TLS | Import depuis un MySQL de test en conteneur | S |
| **CN5** | Écran : CRUD de l'onglet Connecteurs (tableau + pager), test, URL et secret du webhook, lexique « Connecteur ». Conçu avec les skills de design (`frontend-design`, `emil-design-eng`) et passé à `break-ui` (demande de Shan, 2026-11-07) | Parcours e2e : créer, tester, importer ; aucun cas de `break-ui` cassé | M |

Ordre : CN1, CN2, CN3, CN4, CN5.

## 8. Questions ouvertes (techniques)

- **Q-CN1** — Boîte de réception : table dédiée ou réutiliser la file des jobs ? À trancher à CN3
  par le débit visé.
- **Q-CN2** — Un `POST` en lecture (API de recherche) : drapeau explicite sur la requête, à
  ouvrir au premier besoin réel.
