# Veille de réseaux de partenaires — briques noyau (D300–D316)

> Statut : **implémenté hors PW7 (2026-10-19, commit d7f94ceb)** — voir §8 pour
> ce qui est livré, les choix faits en route et les limites connues. Déclencheur :
> le produit Quali-Watch (dépôt `pnex-quali-watch`, PRD v0.1), un SaaS pour
> organismes certificateurs qui contrôlent le réseau des organismes de
> formation qu'ils habilitent. Ce SaaS a **sa propre interface** ; PNEX est
> son moteur (ontologie, collecte, flows, preuves, PDF) et l'outil
> d'enquête des opérateurs.
>
> Règle D190 tenue : rien de propre au CPF n'entre au noyau. Chaque
> décision ci-dessous est générique et sert aussi les autres veilles déjà
> étudiées (usurpation de marque, faux RIB, appels d'offres :
> `pnex-business/lucas/`).

Liens : `ingestion.md` (D317–D322, open data France Compétences dans un
flow), `extension-collector.md` (D254–D284, amendés par D307–D310),
`ontology.md` (D176–D191, D190 packs), `iam.md` (D285–D299, comptes de
service), `notifications.md` (D49–D68), `flow-engine.md`, `media.md`
(versions), `doc-search.md`, `pages.md` (PDF), `api-contract.md`, `mcp.md`,
`ai-assistant.md` §9, `security.md`.

## 1. Le besoin en une phrase

Importer une liste d'organisations (Excel), l'enrichir par des référentiels
publics synchronisés, parcourir leurs sites, garder des preuves datées de
ce qui y est publié, comparer à ce qui est déclaré, puis relancer chaque
organisation par e-mail selon un plan gradué, le tout piloté par une
application externe.

## 2. Ce qui existe et ce qui manque

| Besoin | Existant | Manque |
|---|---|---|
| Modèle organisations / personnes / possession | pack `investigation` (`organization`, `person`, `owns`, `employs`) | champs SIRENE, établissements, mandats de dirigeants |
| Établissements localisés | `map_pin`, page Sites, carte | rattachement à une unité légale |
| Hiérarchie | containment D180 (arbre, parent unique) | une vue d'arbre sur des liens (multi-parents) |
| Import de données | YAML de schéma, `cases_import` | **import d'objets** (Excel, CSV, XML, zip, API) avec clé naturelle |
| Synchronisation planifiée | `inject` cron + `http_fetch` (URL fixe, sans gabarit, réponse en mémoire) | gros fichiers, mapping, différentiel, fermeture des absents |
| Lire une page | agent navigateur `collect_page` (texte, D261 amendé L1) | parcourir un site, HTML de preuve, capture d'écran |
| Versions d'un document | `media_versions` (une version par texte changé) | diff entre deux versions |
| Écrire l'ontologie depuis un flow | `action_trigger`, `page_write`, `range_upsert` | requête, création, mise à jour, liens |
| E-mail | `pnex_notify` SMTP vers des canaux fixes, pièce jointe PDF (`attach_field`) | destinataire tiré du message |
| Webhook sortant | canal `webhook` avec en-tête secret statique | signature du corps, horodatage anti-rejeu |
| Application externe | SDK TS / Rust (`task sdk`), comptes de service étudiés (iam.md) en **lecture seule** V1 | écriture bornée |

## 3. Décisions

### D300 — PNEX moteur d'un SaaS tiers : une org par client, écriture bornée

- Un client du SaaS = **une org PNEX** (R1 : la frontière de sécurité reste
  l'org). Le SaaS tient la correspondance client → org et une **clé
  d'accès de compte de service** par org, côté serveur seulement (iam.md
  §14 : pas de clé dans un navigateur).
- **Amende iam.md** (comptes de service « lecture seule » en V1) : un
  compte de service peut recevoir un rôle personnalisé qui permet
  `ontology.objects:write`, `ontology.links:write` et
  `ontology.actions:invoke` **restreints aux types d'un pack** (portée
  D296), plus `media:read` et `pages:read`. Jamais `iam:*`, jamais de
  deploy/stop de flow, jamais de suppression (D293, D123).
- Le SaaS écrit **par actions d'ontologie** (D183) chaque fois qu'un geste
  métier a des effets (valider un écart, envoyer une relance) : la règle
  vit dans un flow PNEX, le SaaS ne la réimplémente pas. Écriture directe
  d'objets réservée à l'import (D301) et aux réglages.
- Quotas par org (iam.md §14) : les appels du SaaS consomment ceux de
  l'org du client.
- Les opérateurs du SaaS sont membres de chaque org client avec un rôle
  défini à l'onboarding ; leur accès est tracé (D294).

### D301 — Imports déclaratifs vers l'ontologie

Entité org `object_imports` (CRUD `can_write`, API annotée D238) :

- **Source** : fichier envoyé (`xlsx`, `csv`, `json`), ou URL `http(s)`
  (fichier `csv`, `xml`, `json`, éventuellement dans un `zip`), ou API
  JSON paginée (curseur ou page, plafond de pages). Garde de sortie
  identique à `http_fetch` (`egress_address_refused`…).
- **Mapping** déclaratif (YAML, versionné, **livrable dans un pack**) :
  enregistrements (`rows`, chemin XPath simple ou JSONPath pour XML/JSON),
  type cible, clé naturelle, propriétés (colonne → propriété, conversions
  fermées : `trim`, `digits`, `date:<format>`, `enum:{…}`, `money:<devise>`,
  `split:<sep>`), liens dérivés (« la colonne `siret` désigne le
  `map_pin` de même clé »), plusieurs types par ligne autorisés.
  Aucun code dans un mapping : pas de Starlark, pas de gabarit.
- **Modes** : `incremental` (création, mise à jour) ou `snapshot` (en plus,
  les objets ou liens de cette source absents du lot sont **fermés**
  `valid_to = now`, jamais supprimés, A2).
- **Aperçu obligatoire** pour une source fichier : `POST …/dry-run` rend le
  différentiel (créés, modifiés avec ancien → nouveau, fermés) et les
  erreurs par ligne et colonne (jetons de champ habituels) ; `…/apply`
  n'applique que le différentiel d'un aperçu donné, sur la version lue
  (`expected_version`, sinon 409).
- **Provenance** D184 : `import:<import_id>@<run>` sur chaque fait écrit ;
  fiche objet « venu de l'import X le … ».
- Plafonds : 50 000 lignes par run (250 000 depuis l’écriture en bloc, D347), 200 Mo de fichier décompressé, durée
  bornée ; travail exécuté hors requête HTTP, par le worker du backend
  (la fabric de `worker-fabric.md` plus tard, Q-PW3).
- Assistant : l'outil d'aperçu peut être proposé, l'application jamais
  (écriture en masse = humain).

### D302 — Propriété unique : la clé naturelle

`PropertyDef` gagne `unique: bool` (texte ou nombre, implique `indexed`) :
unicité par org × type (index unique partiel PG sur
`properties->>key`, contrôle applicatif sqlite). Erreur de champ
`duplicate` existante. L'import (D301) fait l'upsert sur cette clé ; la
création manuelle d'un doublon est refusée. Ajouter `unique` à un type
déjà peuplé échoue si des doublons existent (liste rendue).

### D303 — Synchronisations planifiées

Un import à source URL ou API peut porter une **récurrence** (vocabulaire
`recurrence` existant, GMAO). Chaque run est journalisé (début, fin,
compteurs du différentiel, erreurs) dans `object_import_runs`.
Échec non silencieux : schéma inattendu (colonne manquante, 0 ligne quand
la précédente en avait N, chute de plus de 30 % des lignes en mode
`snapshot`) → le run est **refusé sans rien écrire**, code
`import-schema-changed` / `import-suspicious-drop`, notification à l'org.
Un run `snapshot` refusé ne ferme jamais rien.

### D304 — Pack `companies_fr` : réutiliser, pas créer

> **Amendé 2026-10-21 (D343, `account-mapping.md`)** : les champs communs, `establishment_of`, `manages` et la hiérarchie `group` passent au pack mondial `companies` ; `companies_fr` 2.0.0 en devient le pack France.

Pack générique (requires `investigation`) :

- unité légale = `organization` étendue (D306) : `siren` (unique),
  forme juridique, NAF, état administratif, dates, NDA, Qualiopi ;
- **établissement = `map_pin`** étendu (`siret` unique, siège, état) :
  chaque établissement apparaît dans la page Sites et sur la carte sans
  travail en plus ; lien temporel `establishment_of` (map_pin →
  organization, une cible) ;
- **dirigeants** = `person` ou `organization` (holding présidente) reliée
  par le lien temporel `manages` (rôle en liste fermée, qualité du
  registre, dates de mandat = validité du lien, source) ;
- mappings livrés : API Recherche d'entreprises (unité légale,
  établissements, dirigeants), liste publique des organismes de formation
  (DGEFP). BODACC et RNE (INPI) en V2.
- **Données personnelles** : nom, prénoms, rôle, dates de mandat ; ni date
  de naissance complète, ni adresse personnelle ; propriétés classées
  `personal` (D298).

Pourquoi pas de type « entreprise » : `organization` porte déjà `owns`
(bénéficiaires effectifs calculés, cases-money K56), `employs`,
`member_of` et tout l'outillage des dossiers ; un second type couperait le
graphe en deux.

### D305 — La hiérarchie est une vue sur des liens, pas du containment

D180 est conservé : le containment reste un arbre à parent unique. Une
personne dirige souvent plusieurs sociétés, une holding a plusieurs
filiales : les mettre en containment forcerait un seul parent et
mentirait. À la place :

- un pack déclare des **hiérarchies** nommées : une liste ordonnée de
  types de liens à suivre, avec leur sens (`establishment_of` inverse,
  `owns`, `manages` inverse) ;
- la fiche objet gagne une vue **Hiérarchie** : arbre calculé à la
  lecture (profondeur ≤ 6, 500 nœuds), un nœud à plusieurs parents
  apparaît sous chacun avec une marque « aussi sous … » ; à une date T
  (liens valides à T, D179) ;
- filtrée par le sous-graphe visible (D297).

### D306 — Un pack peut étendre un type d'un pack requis

Clé `extends:` dans le YAML d'un pack : `{type: organization, properties:
[…]}`, vers un type d'un pack de `requires` **ou** un type système (même
mécanisme que l'extension d'org d'un type système, `ontology.md` §10.1).
Règles : n'ajoute que des propriétés **optionnelles** ; ne retire, ne
renomme, ne rend obligatoire rien ; collision de clé = refus à
l'installation ; désinstaller le pack masque ses propriétés sans effacer
les valeurs. La fiche objet groupe les propriétés par pack.

### D307 — Mission `crawl_site` : découvrir dans un site déclaré

**Amende D254** (« aucune découverte automatique ») : la cible déclarée
peut être un **site** (`web:site:<hôte>`, hôte normalisé comme les règles de navigation ; découverte bornée à l'origine déclarée) ; la découverte reste
bornée à cette origine. Hors origine, rien n'est suivi.

Deux exécutants, un même résultat (registre D309) :

- **Serveur** (défaut, pages publiques) : job du worker, client HTTP
  identifié (`PNEX-Crawler/<version> (+<url de l'instance>)`),
  `robots.txt` lu et respecté, `sitemap.xml` (et index de sitemaps)
  d'abord, puis liens internes en largeur ; une requête par hôte toutes
  les 2 s au plus (plancher compilé), plafonds de pages (défaut 200,
  max 2 000), de profondeur (5) et de taille de page (5 Mo) ; motifs
  d'exclusion ; garde de sortie `http_fetch`. Le texte lisible est
  extrait côté serveur (HTML → texte, sans exécuter de script).
- **Agent navigateur** (sites rendus en JavaScript, ou mode « comme un
  humain ») : même parcours dans l'onglet de la plateforme `web:<origine>`,
  au rythme D258.

**Pertinence** : paramètre de mission `match` (mots, expressions
régulières bornées, liste fermée de moteurs) ; une page qui y répond est
`relevant`, les autres sont seulement listées (URL, titre, empreinte).
Seules les pages pertinentes déclenchent une preuve (D310).

### D308 — Une page pertinente nouvelle ou modifiée déclenche la preuve

Le crawl ne fait pas les preuves lui-même : pour chaque page pertinente
dont l'empreinte du texte a changé (ou jamais capturée), le serveur crée
une tâche `capture_proof` pour un agent navigateur en ligne de l'org (ou
de la ferme de l'opérateur, D300), bail par cible D257. Sans agent en
ligne, la tâche attend (code `ext-no-agent`), le texte serveur reste
disponible.

### D309 — Registre des pages `site_pages`

Comme `social_videos` (D259) : `(org_id, url_normalisée)` unique ;
`site` (cible), `title`, `first_seen_at`, `last_seen_at`, `text_hash`,
`state` (`listed | relevant | captured | gone | blocked`), `state_code`,
`last_proof_id`, `misses` (absences consécutives). Une page absente de 2
crawls successifs passe `gone` (souvent la preuve d'une correction).
Plafond par org (défaut 200 000 lignes). Exposée comme type système
`site_page` pour que les écarts puissent la citer.

### D310 — Preuve horodatée

**Amende l'amendement L1 de D261** (« aucun HTML ne quitte la page ») : le
besoin de preuve visuelle annoncé est là. Une preuve (`capture_proof`)
est un lot de quatre parties, envoyées par l'upload signé existant :

1. **capture d'écran pleine page** (PNG) : défilement +
   `chrome.tabs.captureVisibleTab` recollés, hauteur plafonnée à
   20 000 px, quota de captures du navigateur respecté ; pas de
   `chrome.debugger` (bandeau d'avertissement, permission trop large) ;
2. **HTML** sérialisé du DOM rendu, nettoyé côté extension **et** serveur
   (scripts, iframes, gestionnaires retirés), jamais rendu sur l'origine
   de l'app (R11–R13) : téléchargement seulement ; la capture d'écran
   sert l'affichage ;
3. **texte lisible** (indexé, `doc-search`) ;
4. **manifeste** JSON : URL demandée, URL finale, code HTTP, en-têtes
   utiles (`last-modified`, `etag`), agent, mission, version de
   l'extension, `captured_at` (horloge **serveur** à la réception),
   SHA-256 de chaque partie.

Le manifeste porte aussi le SHA-256 du manifeste précédent de l'org :
**chaîne d'empreintes** append-only (une modification a posteriori casse
la chaîne, vérifiable par `GET …/proofs/verify`). Horodatage qualifié
RFC 3161 par une autorité tierce : en option par org (Q-PW2).
Stockage : médias `image` + `document` reliés, provenance D281
(`source = browser_agent`, `mission_id`, `source_url`), rétention de la
mission (D268). Le PDF de constat (`page_pdf`) embarque la capture et
le manifeste.

### D311 — Diff entre deux versions d'un document

`GET /api/v1/media/{id}/versions/diff?from=n&to=m` : diff de texte par
paragraphes (ajouts, retraits, modifications, positions), calculé à la
demande, plafonné à 1 Mo de texte par côté. UI : bouton « Comparer » dans
l'historique des versions (coquille commune D221). Le message de flow
d'une nouvelle version (`media_source`) porte un résumé du diff.

### D312 — Nœuds de flow ontologie

Trois nœuds, catégorie « Ontologie » (D109), chacun avec sa `NodeDoc` et
le service partagé avec les contrôleurs HTTP (mêmes validations, mêmes
409) :

- `ontology_query` : type, filtres de propriétés (égalité, comparaison de
  dates, `in`), date de validité, tri, `limit` ≤ 500 ; sort une ligne par
  message ou un tableau ;
- `object_upsert` : type, clé naturelle (D302) ou id, propriétés depuis le
  message, `expected_version` optionnelle ; provenance `flow:<id>@<v>` ;
- `link_upsert` : ouvrir ou fermer un lien typé entre deux objets
  (attributs, validité).

Le flow écrit sous **son identité propre** (iam.md §14 : compte de service
du flow, V2), org du flow, droits revérifiés par le backend (le runtime
est non fiable). Plafond d'écritures par run et par minute ; pas
d'écriture sur un type système natif (device, flow…). L'assistant peut
les placer dans un flow non déployé (règles D144 inchangées).

### D313 — Destinataire d'e-mail tiré du message

`pnex_notify` gagne `to_field` (chemin du message) pour les canaux SMTP
**marqués `dynamic_recipients`** par un admin d'org. Garde-fous, car un
destinataire libre fait d'un flow un outil d'envoi en masse :

- contenu = modèle seulement (D52), jamais un corps venu du message brut ;
- plafonds par canal (défaut 200 / jour) et par destinataire (3 / jour) ;
- adresse validée (syntaxe, longueur), pas de liste (un destinataire, plus
  un `cc` du même genre) ;
- journal de livraison D86 avec l'adresse **hachée** et le modèle ;
- expéditeur = le SMTP de l'org (domaine du client, SPF/DKIM chez lui) ;
  PNEX ne relaie pas pour un domaine non configuré.

### D314 — Relances graduées : un patron, pas un moteur

Aucun moteur de relance au noyau. Le patron (livré par le pack) :
un objet « procédure » porte `level` et `next_due` ; un flow
quotidien (`inject` cron → `ontology_query` échéance passée) envoie la
relance suivante (`pnex_notify` D313 + PDF `page_pdf`) puis avance
l'objet (`object_upsert`). Un niveau marqué « validation humaine » crée
une relance `to_validate` au lieu d'envoyer ; l'action `approve_send`
(`action_trigger`) l'envoie. Ce qui manquait au noyau tient dans D312 et
D313.

### D315 — Webhook sortant signé

Le canal `webhook` garde son en-tête secret (compatibilité) et gagne un
mode `hmac` : en-têtes `x-pnex-timestamp` et
`x-pnex-signature: sha256=<HMAC(secret, timestamp + "." + corps)>`,
secret dans le coffre (D54), fenêtre de rejeu documentée (5 min) côté
récepteur. Le SaaS vérifie la signature et l'horodatage.

### D316 — Règles d'un pack et écarts « à valider »

- Les règles métier vivent **dans le pack** : fonctions Starlark (registre
  Automation) pour le déterministe, `llm_text` / `text_decide` pour le
  qualitatif, câblées par les flows du pack. **Amende D190** : un pack
  peut livrer des fonctions et des modèles de flows (non déployés à
  l'installation : l'humain déploie, D123).
- Un constat issu d'un LLM n'est jamais un fait confirmé : l'objet produit
  porte un état « à valider », l'extrait cité mot pour mot et son
  emplacement dans la preuve (locator, comme les affirmations du pack
  `investigation`, `cases.md` K4),
  et la version de la règle.

## 4. Lots

| Lot | Décisions | Sortie | Sert Quali-Watch |
|---|---|---|---|
| PW0 | D302, D306 | propriété unique, `extends:` dans les packs | pack `companies_fr` installable |
| PW1 | D301, D303 | imports fichier + aperçu, puis URL/API planifiés | Excel, France Compétences, CPF, DGEFP |
| PW2 | D304, D305 | pack `companies_fr` + vue Hiérarchie | fiche partenaire |
| PW3 | D307, D309 | crawl serveur + registre `site_pages` | pages pertinentes |
| PW4 | D308, D310, D311 | preuve par l'agent, chaîne d'empreintes, diff | constats datés |
| PW5 | D312 | nœuds ontologie | règles et relances en flows |
| PW6 | D313, D315 | e-mail à destinataire dynamique, webhook HMAC | relances, SaaS notifié |
| PW7 | D300 | compte de service en écriture bornée (après le lot IAM des comptes de service) | SaaS branché |
| PW8 | D316 | fonctions et flows livrés par un pack | pack `quali_watch` complet |

PW0–PW3 ne dépendent pas de l'IAM. PW7 attend I0, I1, I3, I4, I6 de
`iam.md`.

## 5. Sécurité (grille `security.md` §6, points saillants)

- **Sortie réseau** : crawl et imports par URL passent la garde de
  `http_fetch` (pas de loopback, link-local, métadonnées cloud) ; le crawl
  ne suit jamais une redirection hors origine ; tailles et durées
  plafonnées.
- **Parseurs** : xlsx, XML, zip = entrées hostiles (bombe zip, entités
  XML externes interdites, profondeur bornée) ; parseur en Rust sans
  extension réseau.
- **HTML capturé** : jamais servi en `text/html` sur l'origine de l'app
  (R11–R13) ; `Content-Disposition: attachment`.
- **E-mail** (D313) : vecteur de spam / phishing ; réservé aux flows
  déployés, modèle seul, plafonds, journal.
- **Comptes de service** (D300) : écriture bornée aux types du pack,
  jamais d'IAM, de suppression ni d'action physique.
- **Org** depuis le principal (R1) partout ; test viewer → 403 sur toute
  nouvelle route d'écriture (R2).
- **Données personnelles** des dirigeants : classification D298,
  minimisation D304.

## 6. Questions ouvertes

1. **Q-PW1 — Ferme d'agents de l'opérateur** : des agents navigateur
   partagés entre plusieurs orgs (machines de l'opérateur) supposent un
   agent « plateforme » qui sert des missions d'orgs différentes. Le
   modèle actuel lie un agent à une org (appairage). Un agent par org sur
   des machines de l'opérateur suffit-il au départ ? Ou un worker
   Chromium sans interface côté serveur pour la capture d'écran (moins
   de machines, mais une seconde voie de collecte) ?
2. **Q-PW2 — Horodatage qualifié** RFC 3161 (prestataire eIDAS) : sur
   chaque preuve ou à la demande (mise en demeure) ?
3. **Q-PW3 — Imports et fabric de workers** : les imports et le crawl
   tournent-ils sur la fabric (`worker-fabric.md`, non validée) ou sur le
   worker actuel du backend en attendant ?
4. **Q-PW4 — `site_page` système ou type de pack** : le registre est du
   noyau (comme `social_videos`), mais son exposition en type système
   ajoute un adaptateur ; un lien vers le média de preuve suffit-il ?
5. **Q-PW5 — Diff** : par paragraphes suffit-il, ou faut-il un diff visuel
   des captures (régions changées) pour les sites dont le texte bouge peu ?

## 7. Récapitulatif des décisions

| # | Décision | Choix |
|---|---|---|
| D300 | SaaS tiers | une org par client, clé de compte de service côté serveur, écriture bornée aux types d'un pack, gestes métier par actions |
| D301 | Imports | entité `object_imports`, sources fichier / URL / API, mapping YAML sans code, modes `incremental` / `snapshot`, aperçu puis application |
| D302 | Clé naturelle | `PropertyDef.unique`, index unique par org × type |
| D303 | Synchros | récurrence, journal des runs, refus sans écriture sur schéma changé ou chute suspecte |
| D304 | `companies_fr` | `organization` étendue, établissement = `map_pin`, dirigeants par lien temporel `manages`, données minimisées |
| D305 | Hiérarchie | vue d'arbre calculée sur des liens déclarés par le pack, multi-parents, datée ; D180 inchangé |
| D306 | Extension de type | `extends:` vers un type requis ou système, propriétés optionnelles seulement |
| D307 | Crawl | mission `crawl_site` bornée à l'origine déclarée (amende D254), serveur par défaut, agent pour le rendu JS ; pertinence par `match` |
| D308 | Déclenchement | page pertinente nouvelle ou modifiée → tâche `capture_proof` pour un agent |
| D309 | Registre | `site_pages` unique par org × URL, `gone` après 2 absences |
| D310 | Preuve | capture pleine page + HTML nettoyé + texte + manifeste horodaté serveur, chaîne d'empreintes ; amende L1/D261 |
| D311 | Diff | diff de texte entre deux versions de média, API + bouton « Comparer » |
| D312 | Nœuds | `ontology_query`, `object_upsert`, `link_upsert`, identité du flow, service partagé |
| D313 | E-mail | `to_field` sur canal SMTP `dynamic_recipients`, modèle seul, plafonds, journal haché |
| D314 | Relances | patron de pack (objet procédure + flow cron), validation humaine par niveau |
| D315 | Webhook | mode HMAC horodaté |
| D316 | Règles | dans le pack (Starlark + LLM), constat LLM toujours « à valider » ; amende D190 (fonctions et flows livrés par un pack) |

## 8. État d'implémentation (2026-10-19, non commité)

PW0, PW1, PW2, PW3, PW4, PW5, PW6 et PW8 sont codés et testés ; **PW7**
(compte de service en écriture) attend le lot IAM. Tests :
`crates/pnex-backend/tests/partner_watch.rs` (clés naturelles, extensions,
hiérarchie, imports, nœuds d'ontologie, destinataires de message),
`tests/site_crawl.rs` (crawl serveur, registre, `record`, disparition,
preuve scellée, chaîne vérifiée puis cassée), `pnex-node-starlark/tests/pack_rules.rs`
(règles du pack exécutées), tests unitaires des parseurs, du mapping, du
diff, du robots.txt, du nettoyage HTML.

### Choix faits en route

- **D302** : l'unicité est contrôlée par le service avant écriture (pas
  d'index partiel par org × type, qui ferait évaluer des milliers de
  prédicats à chaque insertion). Deux écritures concurrentes de la même
  nouvelle clé peuvent passer (raccourci noté dans le code) ; les imports
  sont sérialisés par org. Rendre une propriété unique sur des doublons :
  409 `ontology-unique-duplicates` avec les premières valeurs.
- **D306** : `extends:` accepte un type d'un pack requis **directement ou
  non** (quali_watch étend `organization` via companies_fr) ; la propriété
  porte `pack` (groupée sous le nom du pack sur la fiche). Pas de
  désinstallation de pack au noyau : le masquage à la désinstallation
  reste à faire avec elle.
- **D301** : langage de mapping (`ontology::import`) : `rows`, `objects`
  (`as`, `type`, `key`, `create`, `update`, `missing: error|skip`, `each`,
  `when`, `title`, `location` pour un `map_pin`, `needs`), `links`
  (`valid_from`, `attributes`, `close_missing`, `create`, `on_conflict`),
  champs `from` / `value` / `concat`, `^chemin` pour remonter à
  l'enregistrement depuis un `each`, `#` pour la position de l'élément
  (à partir de 1) ; conversions fermées `trim`, `digits`, `upper`,
  `lower`, `left:n`, `date:fmt`, `money:XXX`, `split:sep`,
  `split_spaces:n` (un paragraphe par suite d'au moins n blancs),
  `before:sep`, `after:sep`, `max_by:champ` puis `field:champ` (l'élément
  au champ le plus grand, ex. l'année la plus récente), `url_origin`,
  `domain`, `luhn`, `email`,
  `{enum: {…, "*": défaut}}`. Ajouts 2026-10-20 (pack `quali_watch`
  1.2.0, contenu des fiches RS/RNCP) : `needs: <as>` n'écrit l'objet que
  si l'entrée nommée (plus haut) se résout — un registre entier lu sans
  filtre n'enrichit que les certifications de l'org ; `create: false` sur
  un lien ne fait que compléter un lien ouvert ; les attributs et le
  `valid_from` d'un lien qui touche un `each` se lisent dans l'élément.
  Plafonds : 5 000 éléments par `each` et par ligne (une fiche RS liste
  jusqu'à ~4 300 partenaires), 500 000 objets planifiés par run
  (`import-source-too-large`). Source API **par objet** (`per_object` :
  une requête par objet connu, les moins récemment mis à jour d'abord)
  pour les registres interrogés par identifiant ; incompatible avec le
  mode `snapshot`. Aperçu et application tournent en tâche de fond
  (réponse 202 + run à interroger), l'application est une transaction.
- **D303** : runs planifiés à 03:00 UTC du jour de récurrence, avec le
  rôle actuel de l'auteur de l'import (R2) ; refus notifiés dans l'app
  (premier canal websocket de l'org). Fermeture sur `snapshot` refusée
  aussi quand des lignes sont en erreur.
- **D304** : les dirigeants sont dédoublonnés par `registry_id`
  (nom, prénoms, mois de naissance publié) ; la classification `personal`
  (D298) viendra avec l'IAM. Mapping livré : Recherche d'entreprises. Les
  mappings DGEFP, France Compétences et catalogue CPF attendent la
  vérification des dictionnaires de données (Q1 du PRD).
- **D307–D309 / Q-PW4** : le registre `site_pages` reste au noyau **sans**
  type système ; une mission peut écrire ses pages pertinentes comme objets
  d'un type de l'org (`record`, ex. `observed_page`) et relier chaque
  partie de preuve (`proof_link`, ex. `proven_by`). Crawl serveur
  séquentiel (un site après l'autre). L'exécutant agent suit le même
  parcours au rythme D258 (sans lecture du robots.txt : il navigue comme
  un humain).
- **Q-PW3** : imports et crawls tournent sur le worker actuel du backend
  (balayages singleton), pas sur la fabric.
- **D308 / D310** : les tâches `capture_proof` sont des missions créées par
  le serveur seul (refusées à la création manuelle, absentes de la liste
  des missions), une ouverte par page. La capture d'écran exige le droit
  de capture sur le site (sinon `ext-permission-missing`). Le HTML est
  nettoyé par l'agent puis par le serveur et rangé comme document texte
  (téléchargement seulement, mais indexé par la recherche documentaire :
  raccourci noté). Manifeste haché sous forme canonique (clés triées),
  chaîne par org vérifiable par `GET /api/v1/extension/proofs/verify`.
  Horodatage qualifié RFC 3161 : non fait (Q-PW2). Intégration au PDF de
  constat (`page_pdf`) : modèle de page livré par le pack, pas d'insertion
  automatique de la capture.
- **D311 / Q-PW5** : diff par paragraphes (sous-séquence commune bornée à
  2 000 paragraphes par côté après coupe des têtes et queues communes),
  bouton « Comparer les versions » sur la fiche média. Le résumé de diff
  dans le message `media_source` n'est pas fait (ce nœud porte les flux,
  pas les versions de documents).
- **D312** : les nœuds lisent leur configuration dans la version
  **déployée** (jamais dans la requête du runtime), écrivent comme
  `member` avec la provenance `flow:<id>@<version>`, 600 écritures par
  minute et par org (compteur par pod, raccourci noté). `object_upsert`
  gagne `skip_existing` (une règle ne réinitialise jamais un écart revu) ;
  `ontology_query` pose `msg.today` / `msg.now` (pas d'horloge en
  Starlark).
- **D313** : compteurs quotidiens en base (`notify_dynamic_sends`, adresse
  hachée), vérifiés par `POST /internal/notify/recipient` avant chaque
  envoi ; activation du canal réservée aux propriétaires et admins ;
  porte de deploy si un canal du nœud ne l'autorise pas.
- **D316** : un pack livre `functions` et `flows` (brouillons, fonctions
  résolues par nom à l'installation ; une montée de version ne réécrit
  que ceux que l'org n'a pas modifiés, D368). quali_watch livre
  les règles `qw_relance_suivante`, `qw_regle_adm_01`, `qw_lien`, les
  modèles « Quali-Watch · reminders » et « Quali-Watch · rule ADM-01 », et
  les modèles de page « Constat d'écart » et « Rapport partenaire ». Les
  règles WEB-* (LLM, écart `to_validate` avec extrait et repère) ne sont pas
  livrées.

### Reste à faire

- PW7 (comptes de service en écriture bornée), après I0–I6 d'`iam.md`.
- Mappings DGEFP, France Compétences, catalogue CPF (après Q1).
- Règles LLM du pack et leur flow (`llm_text` / `text_decide`).
- RFC 3161 (Q-PW2), insertion automatique de la preuve dans le PDF.
- Passe navigateur réelle : capture pleine page de l'extension (Chrome et
  Firefox) jamais exercée sur un vrai site.
