# Connecteurs : cadre commun, bases, REST générique, webhooks entrants (D791–D797)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-07** (§3). **CN1 à CN4 codés le 2026-11-07/08** (§9) ; CN5 à faire |
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
| Relecture complète (CN2) | Jamais par défaut : `full_every` absent = toujours incrémental |
| Secret des webhooks (CN3) | **Provisoire, à reconfirmer** : la question n'était pas claire le soir du 2026-11-07. Retenu « les deux », qui ne ferme rien : PneX génère le secret (montré une fois, régénérable), l'admin peut coller celui qu'un émetteur impose |

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
- **Entrée hostile** : JSON profondément imbriqué refusé par la limite de récursion du parseur
  (128) ; réponses plafonnées en octets pendant la lecture, avant parsing.
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

- ~~**Q-CN1**~~ — Boîte de réception : table dédiée `connector_inbox` (tranché à CN3, §9).
- **Q-CN2** — Un `POST` en lecture (API de recherche) : drapeau explicite sur la requête, à
  ouvrir au premier besoin réel.

## 9. Avancement

**CN1 (2026-11-07)** : sorte `rest` (migration 000123, colonne `settings`), `RestSettings` et
`RestQuery` dans `pnex_core::connectors`, logique pure dans `connectors/rest.rs` (pagination,
extraction, CSV, en-tête `Link`), envoi dans `services/connectors.rs` (`RestTarget` : client
restreint à l'adresse vérifiée par `resolve`, sans redirection, jeton OAuth2 gardé le temps du run),
boucle des pages dans `services/ingest/query.rs`. Test `an_import_reads_a_paged_rest_api_into_a_table`
(faux serveur local : deux pages, hôte changé, pas de JSON, statut, limite de pages, jeton refusé,
chemin de base piégé). Six codes `connector-rest-*`, fiche `pipelines` complétée.

Écarts à relire :

- **Erreur de statut sans le code HTTP** dans `args` : le journal le garde, l'UI montre seulement
  `connector-rest-status` ou `connector-rest-denied` (401, 403) ;
- **Jeton OAuth2 non rafraîchi** pendant un run (`shortcut:` dans le code) ;
- **Formulaire existant** : il garde la sorte et les réglages d'un connecteur REST mais ne sait pas
  en créer un ; c'est CN5, en attendant l'API seule ;
- **Paramètres de requête** : en clair dans la config de l'import. Une clé d'API va dans
  l'authentification du connecteur (coffre), jamais dans `params`.

**CN2 (2026-11-08)** : `CursorSpec` (`column`, `param`, `start`, `full_every`) sur l'étape
`connector_query`, migration 000124 (`object_imports.cursor`, `cursor_runs`). Choix techniques :

- **Table** : une lecture incrémentale ajoute ses lignes (`append`), une lecture complète la remplace
  (`snapshot`) ; une lecture qui ne trouve rien n'ajoute aucune version ;
- **Synchro** : le planificateur prend la dernière ligne d'une clé comme l'état le plus récent,
  titre compris, sans avertissement de conflit (`Planned::replace`) ;
- **SQL** : `{{cursor}}` dans la requête, remplacé par un littéral que la base source cite elle-même
  (`quote_literal`) ; lignes et maximum lus dans une même transaction `REPEATABLE READ` ; `start`
  obligatoire ;
- **REST** : le paramètre part sur chaque page ; maximum comparé en nombre si les deux valeurs sont
  des nombres, en texte sinon (dates ISO 8601 d'un même format) ;
- **Curseur avancé après le commit de la table** : un run cassé relit sa tranche.

Limite connue : une ligne validée à la source avec une valeur de curseur inférieure au maximum déjà
lu (longue transaction côté source) est manquée ; `full_every` la rattrape.

**CN3 (2026-11-08)** : sorte `webhook` (migration 000125, table `connector_inbox`), route publique
`POST /api/v1/hooks/{id}` (`hooks_receive`, `Public` au registre IAM, exclue du MCP au motif auth),
`POST /connectors/{id}/secret` (`connectors_secret`, `connectors:write`, secret montré une fois).
Choix techniques :

- **Signature** : HMAC-SHA256 (`hmac` 0.12, déjà au lock) vérifiée en temps constant, avant tout
  parsing ; inconnu, sans secret, mal signé ou périmé = même `401 hook-signature-invalid` ;
- **Secret** (provisoire, §3) : généré par `connectors_secret`, ou collé comme mot de passe du
  connecteur quand l'émetteur impose le sien ;
- **Boîte** : 1 Mio par appel, 100 000 appels non lus sur 7 jours par webhook (`503 hook-inbox-full`
  au-delà), appels de plus de 7 jours effacés à la lecture suivante, lus ou non ; 600 appels par minute
  et par adresse cliente (règle `hooks` du limiteur commun) ;
- **Lecture** : pas d'ordonnanceur à part. L'import qui nomme le connecteur lit les appels au-delà du
  dernier id lu (curseur implicite de CN2), par lots de 1 000, 100 000 enregistrements par run ;
  la forme vient de `rest.records` / `rest.columns`. Une boîte vide ne crée aucune version ;
- **Synchro** : toute lecture par connecteur rend `Planned::replace` vrai (la dernière ligne d'une
  clé gagne) ; pour PostgreSQL et REST sans curseur, cela retire seulement l'avertissement
  « conflit » sur une clé répétée dans une même lecture.

Limites : des appels aux champs différents d'un run à l'autre changent les colonnes et cassent
l'ajout à la table (déclarer `rest.columns`) ; le limiteur compte par adresse cliente, pas par
webhook.

**CN4 (2026-11-08)** : sorte `mysql` (migration 000126), feature `mysql` de sqlx, exception
`RUSTSEC-2023-0071` de `deny.toml` élargie avec son motif. TLS exigé à la saisie et revérifié à
la connexion ; session `READ ONLY`, `MAX_EXECUTION_TIME` (MySQL seul, MariaDB garde la limite du
run) ; l'instruction est d'abord préparée seule et doit renvoyer des lignes (refuse un `SET`) ;
lignes lues par le protocole texte, chaque valeur telle que le serveur l'écrit. Curseur : `QUOTE()`
côté source, lignes et maximum dans `START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY`.
Test `an_import_reads_a_mysql_database_under_tls`, ignoré sans `TEST_MYSQL_URL` (conteneur
`mysql:8.4`, commande dans le test), passé le 2026-11-08.
