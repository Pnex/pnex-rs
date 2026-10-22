# Présence web des organisations (D359–D366)

> **Statut : proposé (2026-10-22), questions Q-W1–Q-W4 tranchées par Shan le même jour (§6).** Demande
> de la session `pnex-quali-watch` : trouver le site de chaque organisme
> habilité, le faire valider, puis le crawler chaque semaine pour la
> conformité. Suit `data-pipelines.md` (D344–D358). Liens : `ingestion.md`
> (D317–D322), `partner-watch.md` (D301–D316, D307 crawl, D312 nœuds
> ontologie), `extension-collector.md` (D254), `site-recipes.md`
> (D323–D328), `geo-layers.md` (fournisseurs géo), `secrets.md` (D110–D118),
> `security.md` (R1, R2, R4, R8, R16).

## 1. Constat (code lu le 2026-10-22)

- **Aucune source ouverte ne donne le site** d'un organisme (open data FC
  XML et CSV, catalogue CPF, liste DGEFP : vérifié par la session pair).
  Les données en ont pourtant une trace : `contact_conformite_email`,
  `charge_reseau_email` (pack `quali_watch`) portent souvent le domaine de
  l'organisme.
- **Le type et le lien existent déjà** : `site_web` (`domaine` unique,
  `url_racine`, état de collecte) et `a_pour_site` (organisation → site)
  dans `quali_watch.yaml`. Les reprendre dans un pack générique ne déplace
  aucune donnée : `packs.rs` `hands_over` transfère la propriété d'un type
  quand l'ancien propriétaire se met à dépendre du nouveau.
- **Écrire dans l'ontologie depuis un flow existe** : `object_upsert` et
  `link_upsert` (D312, `services/ontology/flow_nodes.rs`), config lue dans
  la version déployée, acteur `flow:<id>@<v>`, journal O2, budget de
  600 écritures/min. Deux manques pour ce besoin : un lien fermé n'empêche
  pas d'en rouvrir un (le rejet ne tient pas) ; la fermeture n'écrit pas
  d'attributs (impossible de poser `statut: rejete` en fermant).
- **Fournisseurs** : LLM, géo et navigateurs sont **par org** (D119 : la
  plateforme ne fournit pas de LLM). Modèle le plus propre : `geo_providers`
  (`kind`, défaut par capacité, `base_url` contrôlée par `egress`, secret
  à un champ au coffre, route `…/test`). Quota journalier : `count_daily`
  dans Valkey (`flow_ai.rs`).
- **Capture serveur** : `services/site_recipes/cdp.rs` pilote
  `chrome-headless-shell` ou Obscura en CDP (capture, texte), mais seulement
  à travers une recette enregistrée sur un site déclaré. Pas de visite
  ponctuelle d'une URL quelconque.
- **Vision** : `vision_detect` / `vision_caption` ne lisent qu'une clé
  d'image Valkey de caméra ou de média (`frame_key_in_org`).

## 2. Décisions

### D359 — Retrait du nœud `ingest`

Conditionné à L1+ déployé et France Compétences passé en entier sur
l'org 4 (certificateurs → fiches → blocs → partenaires, lancés à la main).

- **Retirés** : le kind `ingest` (graphe, validation, `NodeDoc`, artefact),
  le nœud du runtime (`pnex-node-ui-control/src/ingest.rs`),
  `/internal/flow/ingest` et `PNEX_FLOW_INGEST_URL`, `GET
  /flows/{id}/ingest-steps` (+ outil MCP `flows_ingest_steps`, lignes
  `exposure.rs` / `iam.rs`), l'étape `ontology_import` et
  `imports::open_from_flow` / `flow_run_of`, le rappel « pending », la
  garde de déploiement `ingest::deploy_violations`, le panneau du front
  (palette, inspecteur, catégorie « Ingestion »), les clés ftl
  `flows-ingest-*` / `flows-palette-ingest-*`, la fiche
  `open-data-ingestion.md` (réécrite pour les imports à étapes), le flow
  « Ingestion France Compétences » du pack `quali_watch` (remplacé par les
  `steps:` de ses imports), les tests `open_data_ingestion_chain` et
  `ingest_import_outlives_its_call`. Colonnes `ingest_run_id` /
  `ingest_node_id` de `object_import_runs` supprimées (bêta, D120).
- **Gardés** : `services/ingest` (étapes resolve → filter, staging, purge,
  quota), la table `ingest_steps` (`node_id = import:<id>`), les codes
  `err-ingest-*`, `IngestConfig` (le format des `steps:`).
- **Refus propre** : un flow sauvegardé ou déployé avec un nœud `ingest`
  est refusé au déploiement avec le code `flow-node-retired` (`args` :
  nœud, kind, remplaçant « import à étapes »). Au démarrage du serveur, un
  flow déjà déployé qui le contient est isolé comme les onglets ignorés
  (`flow_error`), jamais de boucle de redémarrage.
- **Flow 9** : désactivé puis supprimé par Shan dans l'UI (D144 : aucun
  outil ne l'arrête). Pas de migration de données.
- Amende D317 et D322 (retirés), D318–D321 inchangés hors flow.

### D360 — Fournisseur de recherche web

- Table `web_search_providers`, calquée sur `geo_providers` : `kind`
  (liste fermée), `base_url` contrôlée par `egress::check_url`, secret à un
  champ au coffre (`SecretConsumerKind::WebSearch`, préfixe
  `web_search/<nom>/`), `timeout_ms`, `rate_limit_per_s`, défaut, route
  `POST …/test`. **Par org** (Q-W1), comme géo et LLM : chaque org pose sa
  clé, la plateforme n'en fournit pas.
- Premiers `kind` : **`serper`** et **`searxng`** (Q-W2, choix de Shan pour
  tester). Pas d'API générale gratuite et officielle en 2026 (Brave sans
  offre gratuite, Google Custom Search fermé aux nouveaux clients et arrêté
  le 2027-01-01). Serper revend des résultats Google et SearXNG interroge
  des moteurs publics : comme pour D254, l'org répond des conditions des
  services qu'elle branche ; l'écran de réglage le rappelle.
  - `serper` : clé au coffre, `POST /search` (`q`, `gl`, `hl`, `num`).
  - `searxng` : instance de l'org (souvent auto-hébergée), clé facultative,
    `GET /search?format=json` (le format JSON doit être activé sur
    l'instance). Une instance sur le réseau du cluster demande que l'opérateur
    autorise son hôte (`PNEX_EGRESS_ALLOW_HOSTS`), comme tout hôte privé.
  - Le `kind` est une liste fermée extensible (Brave, Mojeek plus tard) :
    un fournisseur = un module (requête, traduction de la réponse), sans
    toucher au nœud ni à la table.
- Les candidats gratuits passent **avant** la recherche (D365) : domaine
  des emails déjà connus, et propriété « site officiel » de Wikidata (P856)
  pour les organisations qui y ont un identifiant (SIREN P1616). Exclus par principe : tout ce qui gratte un
  moteur sans accord (Serper et équivalents revendent des pages Google ;
  SearXNG interroge les moteurs à leur insu). Un `kind` = un module de
  quelques dizaines de lignes (requête, traduction de la réponse).
- Quota : `count_daily("web_search", org)`, plafond `web_search.daily_calls`
  (défaut 500), refus `web-search-quota` ; erreurs fournisseur codées
  (`web-search-unavailable`, `web-search-denied`).

### D361 — Nœud `web_search`

Entrée `msg.payload.query` (texte, ≤ 400 caractères) ; config :
fournisseur (ou défaut), `limit` (≤ 20), `country`, `lang`, exclusions de
domaines (annuaires et réseaux sociaux par défaut). Sortie :
`payload.results = [{url, title, snippet, rank, domain}]`, plus
`payload.query` et `payload.provider`. **N'écrit rien.** Appel par
`/internal/flow/web-search`, config lue dans la version déployée
(`flow_nodes::deployed`), org tirée du nœud (R1). Rien ne quitte le
serveur sauf la requête.

### D362 — `link_upsert` : rejet durable et fermeture motivée

Deux options sur la config existante, pas de nouveau nœud :

- `skip_closed: bool` : si un lien fermé existe déjà pour la même paire,
  le nœud le renvoie tel quel (`created: false`, `closed: true`) au lieu
  d'en ouvrir un nouveau. C'est ce qui empêche de reproposer un candidat
  rejeté.
- en fermeture (`close: true`), les `attributes` du message sont fusionnés
  avant la fermeture, dans la même transaction.

`object_upsert` reste tel quel (créer le `site_web` par `domaine`).

### D363 — Pack générique `presence_web`

Pack de référence `presence_web` (requiert `entreprises`), que
`quali_watch` et `prospection_b2b` requièrent. Il **reprend** `site_web` et
`a_pour_site` de `quali_watch` (même clé, même sens, transfert par
`hands_over`, aucune donnée déplacée) ; on ne crée pas de `site_de` :
l'inverse s'affiche déjà « est le site de ».

Attributs ajoutés sur `a_pour_site` :

| Clé | Type | Rôle |
|---|---|---|
| `statut` | enum `propose`, `confirme`, `rejete` (indexé) | seul `confirme` part au crawl |
| `score` | nombre 0–100 | |
| `preuve_siren`, `preuve_nda`, `preuve_adresse` | bool | ce que la visite a trouvé |
| `avis_llm` | texte ≤ 2 000 | justification du modèle, si appelé |
| `requete`, `rang`, `fournisseur` | texte, nombre, texte | provenance du candidat |
| `source` | enum `email`, `recherche`, `manuel` | |
| `valide_par` | texte | **Q-W3** |

Un candidat rejeté = lien fermé avec `statut: rejete` (D362) ; l'historique
O2 garde qui a fait quoi. `site_web.domaine` est la clé ; `etat_collecte`
reste. La page « Sites web » et `web:site:<hôte>` : un `site_web` confirmé
devient une cible de crawl D307 (`web:site:<domaine>`), sans nouveau
registre.

### D364 — Visite ponctuelle d'un candidat (amende D254 et D307)

**Amendement** : un site non déclaré peut être **visité pour
l'identifier**, jamais crawlé. Visite = au plus 3 requêtes sur l'origine
du candidat : l'URL donnée, `robots.txt`, la page de mentions légales
(premier lien interne dont le texte ou le chemin correspond à une liste
fermée : « mentions légales », « legal », « CGV », « qui sommes-nous »).
Robots respecté, client identifié (`PNEX-Crawler`), plancher de 2 s par
hôte, egress gardé. Le crawl D307 ne démarre qu'après `statut: confirme`.

Nœud **`web_visit`** : entrée `msg.payload.url` ; sortie `final_url`,
`status`, `title`, `text` (accueil et mentions, ≤ 50 000 caractères
chacun), `legal_url`, et `screenshot` (**Q-W4**). Exécuté par le serveur :
texte par le client HTTP du crawl (`crawl::html::read`), capture par CDP
(`site_recipes/cdp.rs`) si un navigateur est configuré pour l'org, sinon
sans capture.

### D365 — Flow d'identification livré par le pack

`ontology_query` (organisations sans `a_pour_site` confirmé, par lots) →
candidats sans recherche d'abord (domaine des emails de l'org hors
webmails, liste fermée) → `web_search` (raison sociale + ville) →
`web_visit` sur les 3 premiers domaines non rejetés → score déterministe
(fonction Starlark du pack : SIREN/SIRET ou NDA dans les mentions, code
postal et voie comparés aux établissements SIRENE) → si score moyen,
`llm_*` sur texte et capture (`Msg::UserImage`) → `object_upsert`
(`site_web`) + `link_upsert` (`a_pour_site`, `skip_closed`).

Seuils (réglage du pack) : ≥ 80 `confirme` directement, 40–79 `propose`
(validation humaine), < 40 rien écrit. La capture de la visite est
conservée comme asset média de l'org rattaché au `site_web` (Q-W4) : la
page de validation l'affiche, la vision et le lab la reliront plus tard
sans copie (quota de stockage de l'org). Le flow est livré non déployé :
c'est l'humain qui le déploie (D144).

### D366 — Validation humaine

Page **« Sites à valider »** (Q-W3) : organisme, candidat, capture,
preuves trouvées, score, boutons Confirmer et Rejeter ; liste des liens
`a_pour_site` ouverts au `statut: propose`. `valide_par` = nom affiché de
l'utilisateur (pas d'identifiant, R4). Le geste écrit par le service partagé
(`links::update` puis `close` pour un rejet), acteur = l'utilisateur ;
permission `ontology:write`.

## 3. Plus tard (hors de ces lots)

- `vision_detect` / `vision_caption` sur une image stockée (capture de
  visite ou de preuve) : élargir `frame_key_in_org` à une clé d'image
  d'objet, ou un `image_source` par asset.
- Import des captures d'un crawl dans le vision lab (projet, images), pour
  entraîner la détection des logos réglementés (Qualiopi, France
  Compétences, MCF, Marianne, certificateur).
- Règles de conformité du crawl hebdomadaire (texte d'abord).

## 4. Sécurité (grille `security.md` §6)

- **R1** : org tirée du nœud déployé (internes) ou du principal (routes).
- **R2** : `web_search_providers:read|write|test` au registre IAM, test
  403 par route ; validation sous `ontology:write`.
- **R4 / R16** : clé du fournisseur au coffre, jamais relue ; DTO sans
  secret ; champ dans `REDACTED` (MCP).
- **R8** : recherche et visite par le client gardé ; URL candidate
  refusée si privée, IP littérale ou schéma autre que http(s).
- **Contenu hostile** : texte et capture d'un site inconnu = données non
  fiables. Le texte passe au LLM comme donnée (pas d'instruction suivie,
  sortie bornée à un schéma `text_decide`) ; la capture n'est jamais
  rendue sur l'origine de l'app (R11–R13).
- **MCP** : lecture des fournisseurs exposée ; écriture et test `EXCLUDED`
  (secret, sortant). L'assistant ne lance ni recherche ni visite.

## 5. Lots

| Lot | Contenu | Prérequis | Taille |
|---|---|---|---|
| **W0** | D359 retrait du nœud `ingest` | L1+ déployé, FC passé sur l'org 4 | M (beaucoup de fichiers, peu de logique) |
| **W1** | D360–D361 fournisseurs + nœud `web_search`, page de réglage | — | M |
| **W2** | D362–D363 options de `link_upsert`, pack `presence_web`, montée de `quali_watch` | — | S |
| **W3** | D364 nœud `web_visit` (texte, puis capture) | — | M |
| **W4** | D365–D366 flow du pack, fonction de score, surface de validation | W1–W3 | M |

W1, W2 et W3 sont indépendants ; W0 attend le déploiement.

**Avancement (2026-10-22, non commité)** — **W1 codé** :

- table `web_search_providers` (migration 000044), service
  `services/web_search.rs` (CRUD au modèle des navigateurs, clé au coffre
  `web_search/<nom>/key`, consommateur `web-search-provider`), routes
  `GET|POST /api/v1/web-search/providers`, `PUT|DELETE …/{id}`,
  `POST …/{id}/test` ; permissions `web-search:read` (lecteur),
  `web-search:write|delete|test` (admin) ; MCP : liste exposée, écriture
  et test `EXCLUDED` (secret, destructif, sortant) ;
- `kind` `serper` (clé obligatoire, adresse par défaut) et `searxng`
  (adresse obligatoire, clé facultative en Bearer, sortie JSON) ;
- nœud `web_search` (cœur `pnex_core::web_search`, runtime
  `pnex-web-search`, route interne `/internal/flow/web-search`, options lues
  dans la version déployée) ; quota `settings.web_search.daily_calls`
  (500 par défaut, compteur Valkey `web-search`) ; ports résultats / échec ;
- section « Recherche web » de la page Organisation, nœud dans la palette
  (catégorie Ingestion, à côté de la règle de site), fiche `assistant-kb`
  `web-search.md`, ligne au lexique et règle de garde (« moteur de
  recherche » refusé).

**Avancement (2026-10-22, non commité)** — **W2, W3 (texte), W4 codés** :

- **W2** : `link_upsert` gagne `skip_closed` (une paire déjà fermée répond
  son dernier lien fermé, jamais rouvert) ; `close` fusionne d'abord les
  attributs du message (motif, qui a décidé). Les deux écritures se font
  l'une après l'autre, pas dans une seule transaction (chacune journalise ;
  le message suivant ferme si la seconde a échoué). Pack **`presence_web`**
  1.0.0 (`requires: [entreprises]`) : `site_web`, `a_pour_site` et ses
  attributs D363 (statut coloré D339), `organization.site_recherche_le` ;
  `quali_watch` 1.3.0 et `prospection_b2b` en dépendent. **La passation
  couvre maintenant les types d'objets** (`types::set_pack`), en plus des
  propriétés et des types de liens. Test :
  `presence_web_takes_websites_over` (org en `quali_watch` 1.2.0 → 1.3.0).
- **W3** : nœud `web_visit` (cœur `pnex_core::web_visit`, runtime
  `pnex-web-visit`, route `/internal/flow/web-visit`). Il fait trois requêtes
  au plus avec le client du parcours D307 (identifié, egress gardé,
  redirections hors origine non suivies) : `robots.txt` (absent = tout
  permis), la page, puis les mentions légales (premier lien interne dont le
  chemin est dans `LEGAL_PATHS`). Un plancher de 2 s par hôte est tenu par
  processus. Quota `settings.web_visit.daily_calls` (1 000 par jour).
  Quatre codes `web-visit-*`. Test `tests/web_visit.rs`. **Capture non
  faite** (Q-W4) : elle reste à brancher sur CDP et l'asset média.
- **W4** : opérateur de filtre `missing` (propriété absente) pour
  `ontology_query`.
  - Flow « Web presence · find websites » livré en brouillon. Chaque nuit,
    il prend au plus 50 organisations jamais traitées.
    `pw_preparer` produit trois sorties : la requête, le marquage
    `site_recherche_le`, et un candidat tiré du domaine e-mail (hors
    webmails). Viennent ensuite `web_search` (3 résultats), `pw_resultats`
    (un message par résultat), `web_visit` et `pw_score`. Le score est
    déterministe : SIREN 60, NDA 30, domaine de ses e-mails 30, nom 10.
    À partir de 80 le site est `confirme`, de 40 à 79 `propose`, en
    dessous rien n'est écrit. Enfin `object_upsert` crée le `site_web`
    par `domaine` et `link_upsert` pose `a_pour_site` avec `skip_closed`.
  - L'étape LLM pour les scores moyens n'est pas faite : un score moyen
    va directement à l'humain.
  - Section **« Sites à valider »** en tête de la page Sites web, appuyée
    sur `GET /api/v1/web-presence/candidates` (lecture `ontology:read`,
    exposé au MCP) et `POST …/{id}/decision` (`confirme` | `rejete`,
    `ontology:write`, exclu du MCP sous « sign-off »).
  - `valide_par` = nom affiché, « — » à défaut, jamais l'e-mail. Le
    domaine candidat est affiché en texte, jamais en lien cliquable.
  - Tests : Starlark (`pack_rules.rs`, 2 tests) et
    `presence_web_takes_websites_over` (liste, lecteur 403, rejet
    définitif, 404 sur un lien fermé).

Limites connues :
- `link_upsert` sur un lien **ouvert** met ses attributs à jour : un site
  déjà confirmé retrouvé par le flow (organisation remise à zéro à la
  main) repasserait en `propose`.
- Seul `confirme` doit partir au parcours D307 : le filtre côté parcours
  n'est pas codé (les sites déclarés sans statut restent parcourus).

## 6. Questions

Tranchées par Shan le 2026-10-22 :

- **Q-W1** — Portée du fournisseur : **org seule**, comme géo et LLM
  (D119).
- **Q-W2** — Premiers fournisseurs : **Serper et SearXNG** pour tester ;
  architecture ouverte à d'autres `kind`.
- **Q-W3** — Validation : **page « Sites à valider »**, `valide_par` = nom
  affiché.
- **Q-W4** — Capture : **asset média de l'org**, rattaché au `site_web`.
