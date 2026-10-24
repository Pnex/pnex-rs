# Contrôle d'un réseau de partenaires de bout en bout (D402–D414)

> **Statut : proposé (2026-10-24)**, demande de la session `pnex-quali-watch`
> (Shan : « coder tout ce qu'il faut […] et anticiper les futurs dév pour
> vérifier aussi les noms, la conformité, les règles de navigation »).
> Suit `partner-watch.md` (D300–D316), `site-recipes.md` (D323–D328),
> `web-presence.md` (D359–D366), `data-pipelines.md` (D344–D358) et
> `proposals.md` (D371–D377). Côté produit : PRD Quali-Watch §5.3, §5.4,
> §5.7 et §8, `navigation.md`, `ingestion.md`. Défauts retenus en attendant
> Shan : §5.

## 1. Constat (dev.pnex.io, org 4, image de 20688563)

- Le réseau du pilote est en base : certification RS0101, certificateur
  CERTIF EXEMPLE, 22 partenaires reliés par `habilite_pour`. Les
  données France Compétences sont complètes et leurs récurrences sont posées.
- L'enrichissement rate sa cible. L'import SIRENE `per_object` a pris
  200 organisations dans l'ordre des SIREN, et aucune n'est du réseau. Il a
  aussi perdu 6 établissements sans coordonnées (`location: required`).
- Rien ne relie un site confirmé au parcours D307. Une mission `crawl_site`
  se crée à la main, avec son `match` à la main.
- Les preuves attendent un agent navigateur (D308). Sans extension en
  ligne, rien n'est prouvé sur dev, même avec le navigateur du cluster.
- Seule la règle ADM-01 existe, et elle lit toutes les organisations cessées
  de l'org, réseau ou non.
- Les données ouvertes manquantes existent, vérifiées le 2026-10-24 :
  - DGEFP (liste publique des OF, opendatasoft) s'interroge par SIREN. OF EXEMPLE (100000009) y figure : NDA 11000000000, Qualiopi « Actions de
    formations ».
  - Le catalogue Mon Compte Formation (Caisse des Dépôts, opendatasoft)
    **porte le SIRET** de l'organisme (Q1 du PRD tranchée). Il s'interroge
    par certification : RS0101 = 83 lignes, 26 formations, 20 organismes,
    dont OF EXEMPLE et OF AUDIT, et des organismes absents du réseau (OF HORS RESEAU,
    OF AUTRE SITE…), c'est-à-dire des cas CPF-01 à instruire.

## 2. Principe

Le noyau gagne **cinq primitives génériques**. Tout le métier reste dans le
pack (D190, D316) : imports, fonctions Starlark et flows livrés en brouillon,
que l'humain déploie (D123, D144).

1. une requête qui sait parcourir un réseau et ramener les liens (D402) ;
2. un enrichissement borné au réseau (D403) ;
3. un établissement sans coordonnées qui n'arrête plus un import (D406) ;
4. un parcours de site lancé par un flow (D407) ;
5. une preuve prise par le serveur quand aucun agent n'est là (D408).

## 3. Décisions

### D402 — `ontology_query` parcourt le réseau et joint les liens

Le nœud expose ce que la requête D185 sait déjà faire :

- **`traverse`** : liste de sauts `{link_type, direction}`, 3 au plus.
  Exemple : `certification` avec `code in [RS0101]`, puis `habilite_pour`
  entrant, donne les partenaires.
- **`with_links`** : types de liens joints à chaque ligne, dans les deux
  sens. Chaque ligne porte `links.<type>`, une liste de
  `{direction, attributes, valid_from, other: {id, type, title, properties}}`.
  On n'y met que les liens ouverts à l'instant de la requête, au plus 200 par
  ligne et par type.

Une règle lit ainsi en un message le partenaire, ses habilitations (état
FC, dates, périmètre) et ses sites, sans jointure en Starlark. Rien d'autre
ne change : même service que l'API, org du nœud déployé (R1), lecture
seule, `limit` ≤ 500. Le message garde ses champs, donc une sortie `array`
(le réseau en un message) puis une sortie `each` (une ligne par message)
font une jointure simple. Le contexte reste dans `msg`.

### D403 — Enrichissement borné au réseau : `per_object.linked`

`per_object` gagne `linked: [<type de lien>…]` : seuls les objets qui ont un
lien **ouvert** de l'un de ces types sont interrogés, dans un sens ou dans
l'autre. Avec `linked: [habilite_pour, porte]`, on couvre les partenaires et
le certificateur. Rien d'autre ne passe, même si l'org connaît
200 000 sociétés.

- La règle se combine avec `only`. Le curseur tournant ne change pas.
- Le pack `quali_watch` livre ses propres imports SIRENE et DGEFP bornés au
  réseau. L'import générique d'`entreprises_fr` reste tel quel pour les
  autres usages.

### D404 — DGEFP : NDA et Qualiopi par SIREN

L'import « DGEFP · organismes de formation » est une source API
`per_object`. Il appelle
`…/liste-publique-des-of-v2/records?refine=siren:{value}`, sur le réseau
seulement (D403) et chaque semaine.

Il écrit sur `organization` des propriétés qui existent déjà
(`entreprises_fr`) :

- `nda` ;
- `qualiopi` (vrai si une catégorie est certifiée) ;
- `qualiopi_categories` : texte trié, `actions;bilans;vae;apprentissage` ;
- `qualiopi_maj` : date de la dernière déclaration.

Un organisme absent de la liste n'est pas modifié : l'import ne conclut rien
d'une absence. La règle ADM-02 lit la date de passage (`qualiopi_maj`)
avant de conclure.

### D405 — Catalogue CPF par certification

Deux imports sont livrés (RS et RNCP), tous deux `per_object` sur
`certification` et bornés par `only: {property: repertoire, …}`. Ils
appellent `…/moncompteformation_catalogueformation/exports/json?refine=type_referentiel:RS&refine=code_inventaire:{value}`
(ou `code_rncp` pour le RNCP). L'export rend toutes les lignes sans
pagination.

- `certification` gagne la propriété `numero` (`0101`). Elle est écrite par
  les imports France Compétences, car la valeur interrogée doit être un
  identifiant nu.
- `offre_cpf` est clé par `numero_formation`, à raison d'un objet par
  formation et non par session. Ses propriétés sont l'intitulé, le prix
  moyen, les heures, le SIRET, `a_distance` (sessions à distance > 0) et
  `vue_le` (`date_extract`).
- `offre_de` relie l'offre à l'organisation par SIREN. Une organisation
  inconnue est **créée** (titre `nom_of`) : c'est l'organisme non habilité
  de CPF-01.
- `porte_sur` relie l'offre à la certification.
- Le mode est `incremental`. Une offre retirée du catalogue reste avec son
  `vue_le` ancien, et une règle la lit comme « plus vue depuis ».

### D406 — Établissement sans coordonnées : avertissement, puis géocodage

**MVP (livré)** : un `map_pin` à créer sans latitude ni longitude n'est pas
créé, comme avant, mais la ligne passe en **avertissement**
(`location: required`) au lieu d'une erreur. Le run se termine sans erreur,
l'unité légale est enrichie, et un run `snapshot` n'est plus refusé pour un
siège non géocodé.

**V2** : `location` d'un mapping gagnera `geocode: <champ adresse>`. L'import
demandera alors l'adresse au fournisseur géo de l'org (Photon, capacité
`geocode`), par le proxy géo (même cache, même garde de sortie, 200
adresses par run au plus). Il faut pour cela faire passer la configuration
et le coffre jusqu'aux imports, qui ne connaissent aujourd'hui que la base
(aperçu, application, runs planifiés, étapes) : c'est un lot à part.

### D407 — Nœud `site_crawl` : le parcours D307 lancé par un flow

Il faut savoir **quand** un site est parcouru, et **pourquoi** une page est
pertinente. C'est le flow qui porte ces deux réponses :

```
inject (hebdo) → ontology_query (réseau, with_links a_pour_site, habilite_pour)
 → qw_cibles (une cible par site : url + match + contexte)
 → site_crawl → règles WEB-* → object_upsert (ecart) → liens
```

- **Entrée.** `msg.payload.url` (le site) et `msg.payload.match`, une liste
  de `{kind: word|phrase|regex, value}` (50 au plus, comme D307). La fonction
  du pack y met les codes RS/RNCP du partenaire, les intitulés, le nom du
  certificateur et le lexique CPF.
- **Config du nœud** : `max_pages` (défaut 200), `exclude`, `proofs`
  (défaut oui) et `record` (le type `page_observee` et ses propriétés, comme
  la mission).
- **Exécution.** Le serveur tient **une mission `crawl_site` par site**, de
  clé org + origine : il la crée au premier passage, puis met à jour ses
  paramètres avec ceux du message. Il joue alors le parcours D307 serveur
  comme le balai, sans rien changer : registre D309, disparition après deux
  absences, demande de preuve D308, `record`. La mission est marquée
  `flow:<id>` et le balai horaire la saute : le flow seul la déclenche.
  Un appel attendu prend 30 min au plus, un site à la fois (comme
  `site_recipe`).
- **Sorties.**
  - Port 0 : une **page pertinente nouvelle ou modifiée** par message, le
    message d'entrée cloné avec `payload = {url, title, text (≤ 50 000 car.),
    page_id, object_id, changed}`.
  - Port 1 : le **bilan du site**, `{url, pages, relevant, truncated,
    blocked}`, qui sert à COL-01.
  - Port 2 : le refus (`msg.error = {code, args}`).
- La garde de déploiement exige `record.type` existant. Codes : `site-crawl-*`.
- **Sécurité.** L'URL est revérifiée (R8 : schéma http(s), pas d'IP
  littérale ni privée). Le parcours reste borné à son origine (D307), et le
  plancher de 2 s par hôte est compilé.

Un site « confirmé » est un lien `a_pour_site` **ouvert**. Depuis D376, un
candidat moyen est une proposition, pas un lien. La limite connue de
web-presence §5 (un site non confirmé parcouru) disparaît donc.

### D408 — Preuve prise par le serveur (amende D308)

Une tâche `capture_proof` qui attend sans agent en ligne (`ext-no-agent`)
depuis plus de 5 min est prise par le **serveur**. Le navigateur est celui
de l'org par défaut, sinon celui de la plateforme (même résolution que les
règles de site, D325).

- **Ce qui est pris** : une capture PNG (pleine page si le moteur sait
  `captureBeyondViewport`, sinon la fenêtre, et le manifeste le dit), le
  HTML du DOM rendu, nettoyé par `clean_html`, et le texte lisible.
- **Scellement** : le même que pour une preuve d'agent, dans la même chaîne
  d'empreintes. Le manifeste porte `agent = server:<moteur>` et
  `executor = server`. `verify` ne change pas.
- **Garde** : la page est gardée comme dans une règle de site (`guard_page`,
  navigation restée sur l'hôte, R8), avec une page par 2 s et par hôte.
  Une preuve serveur qui échoue laisse la tâche à un agent (rien n'est
  perdu) et est retentée au passage suivant, 3 fois au plus.
- Le choix agent ou serveur ne change ni la valeur de la preuve ni la
  chaîne. Le PRD veut une page vue « comme par un humain » : l'agent reste
  préféré quand il est en ligne.

### D409 — Écarts : déterministe `propose`, textuel ou LLM `a_valider`

- Une règle **déterministe sur des faits de registre** (SIRENE, DGEFP, FC,
  CPF) écrit l'écart `propose`.
- Une règle qui lit **le texte d'une page** écrit l'écart `a_valider`, même
  quand elle est déterministe : une page se lit mal (menu, archive, mention
  négative). Il en va de même pour une règle **LLM**.
- L'**extrait** est cité mot pour mot. La fonction du pack retrouve la
  citation dans le texte de la page (espaces normalisés). Une citation du
  LLM introuvable n'écrit rien (D316, D328).
- La **clé** d'un écart vaut `<règle>:<siren>:<code certif>[:<url>]`, avec
  `skip_existing` : un écart revu par un humain n'est jamais réécrit.
- Les **liens** sont `concerne` (organisation), `sur` (certification) et
  `constate_dans` (page observée ou offre CPF). L'objet `regle` et le lien
  `selon` attendent les réglages par client (Q-N2). L'écart porte en
  attendant son code (titre, clé) et `regle_version`.
- **Q-P1 maintenu** : un écart n'est pas une proposition (`proposals`).
  C'est un constat daté qui suit son propre cycle (validation, relance,
  clôture), affiché dans l'écran « Écarts à valider » du SaaS.

### D410 — Catalogue des règles livré par le pack

Le pack lit deux réglages (D368) :

- `certifications`, les codes contrôlés (vide = toutes celles que porte un
  certificateur de l'org) ;
- `regles_inactives`, les codes des règles éteintes.

| Code | Règle | Constat | Nature | Gravité | Lot |
|---|---|---|---|---|---|
| ADM-01 | Partenaire cessé encore habilité | `etat_administratif = cesse` × `habilite_pour` ouvert | registre | critique | **MVP** |
| ADM-02 | Qualiopi absent ou sans « actions de formation » | DGEFP × habilité | registre | critique | **MVP** |
| CPF-01 | Offre CPF d'un organisme non habilité | `offre_de` × aucun `habilite_pour` ouvert sur la certification | registre | critique | **MVP** |
| CPF-02 | Offre CPF hors périmètre (habilité « évaluation » seulement) | `perimetre = evaluation` × offre | registre | majeur | **MVP** |
| CPF-03 | Offre CPF après la fin d'habilitation | `date_fin` / `etat_fc = inactif` × offre vue après | registre | critique | **MVP** |
| REG-01 | Habilitation connue hors open data | `habilite_pour.declaree_fc` ≠ vrai | registre | info | **MVP** (sens open data → client : V2, il faut la liste du client) |
| REG-02 | Habilitation inactive FC mais partenaire actif | `etat_fc = inactif` × (offre CPF ou page pertinente) | registre | critique | **MVP** |
| COL-01 | Site inaccessible, bloqué, ou sans page pertinente | bilan `site_crawl` | collecte | info | **MVP** |
| WEB-02 | Code absent (intitulé cité sans le code) ou faux (autre code à côté de l'intitulé) | page | texte | majeur | **MVP** |
| WEB-08 | Code d'une certification du client cité sans habilitation ouverte et active | page × habilitations | texte | critique | **MVP** |
| WEB-04 | « Éligible CPF » sur une certification non éligible pour ce partenaire | lexique CPF × WEB-08 ou certification non active | texte | critique | **MVP** |
| WEB-01 | Intitulé inexact | code cité sans l'intitulé normalisé : extrait autour du code | texte | majeur | **MVP** (texte), V2 (LLM sur l'extrait) |
| WEB-03 | Certificateur non cité, ou partenaire présenté comme certificateur | nom du certificateur absent d'une page qui cite le code (texte) ; « notre certification », « certifié par nous » (LLM) | texte + LLM | majeur | **MVP** (texte), V2 (LLM) |
| NOM-01 | Nom affiché ≠ raison sociale, sigle, nom commercial ou enseigne | mentions légales (`web_visit`) × SIRENE | texte | mineur | V2 |
| WEB-05 | Promesse interdite (« gratuit », cadeau contre inscription) | lexique fermé + LLM | texte + LLM | critique | **MVP** (lexique), V2 (LLM) |
| WEB-06 | Qualiopi affiché pour une catégorie non certifiée | texte « Qualiopi » × « bilan », « VAE », « apprentissage » × DGEFP ; logo : vision | texte, vision | mineur | **MVP** (texte), V2 (vision) |
| WEB-07 | Modalités ou prérequis contraires à la fiche | page × `prerequis`, jury | LLM | majeur | V2 |
| LIEU-01 | Lieu de formation annoncé sans établissement déclaré à proximité | page ou offre × établissements (géo) | géo | mineur | V2 (D412) |
| CPF-04 | Prix ou durée du site ≠ offre CPF | page × offre | texte | mineur | V2 |
| PROG-01/02/04 | Couverture, heures, modalités du programme | tableau croisé | LLM puis calcul | majeur | V2 (D413) |

Les règles « texte » du MVP sont toutes déterministes : listes fermées,
normalisation par `fold` (minuscules, sans accents). Elles marchent donc sur
l'org 4 **sans fournisseur LLM**. Les branches LLM (WEB-01, WEB-03,
WEB-05 et WEB-07, toutes en V2) se branchent sur `llm_text` : sans fournisseur, le
nœud échoue sur son port d'échec et la partie déterministe a déjà écrit son
écart.

### D411 — Contrôle des noms (anticipé)

Trois noms à rapprocher : celui du **registre** (SIRENE : `nom_complet`,
`sigle`, enseignes et nom commercial du siège, ajoutés au mapping), celui de
**France Compétences** (`Nom_Partenaire`, gardé comme alias à l'import) et
celui du **site** (titre, mentions légales lues par `web_visit` D364).

- Une seule normalisation : `fold`, sans formes juridiques (SAS, SARL, EURL…),
  ponctuation repliée.
- NOM-01 (V2) : aucun nom du site ne correspond à l'un des noms du registre.
- WEB-03 (texte, MVP) : la page cite un code du client sans le nom du
  certificateur (raison sociale ou sigle).
- Les écarts de nom sont `a_valider` : un nom commercial légitime ne figure
  pas toujours au registre.

### D412 — Géographie (anticipé)

Fournisseurs géo de l'org, déclarés par la session pair dès que le chart
est déployé : Photon pour `geocode` et `reverse`, GraphHopper pour `route`.

1. **Sièges sans coordonnées** : D406 (avertissement MVP, géocodage V2).
2. **Score de site (D365)** : l'adresse des mentions légales est géocodée
   par `geo_geocode` et comparée aux établissements SIRENE. À moins de
   500 m : +30, à la place de la comparaison du code postal en texte. C'est
   un changement de `pw_score` dans `presence_web` 1.6.0 (V2, après le
   déploiement de la géo).
3. **LIEU-01** (V2) : les villes de formation annoncées (page : liste
   fermée « à <Ville> », « en présentiel à » ; offre CPF : département) sont
   géocodées, puis comparées aux établissements du partenaire. Un lieu à plus
   de 50 km de tout établissement donne un écart `a_valider` (mineur ; une
   salle louée est légale). Le routage ne sert pas.
4. **Carte du réseau** : les établissements sont des `map_pin` (D35). La
   hiérarchie `reseau` du pack les montre déjà sur la carte (POI-first) ; il
   reste à ajouter le saut `etablissement_de` à la hiérarchie (MVP, une
   ligne de pack).

### D413 — Navigation et programmes (anticipé, V2)

Le studio et le nœud `site_recipe` (D323–D327) sont livrés. Ce qui manque
pour le tableau croisé :

- une règle de navigation par partenaire (« programme-RS0101 »), écrite dans
  le studio, une fois par site, par un humain aidé de l'assistant ;
- un flow « programmes » livré par le pack : `site_recipe` → `media`
  (extrait texte ou PDF versionné) → `llm_extract` (N3, D328) vers
  `programme` / `module` / `couvre`, écrit `a_valider`, citation retrouvée
  par le serveur ;
- les règles PROG-01 (compétence non couverte), PROG-02 (heures sous la
  référence, tolérance du réglage) et PROG-04 (modalités), en Starlark, sur
  la vue `certification → competence ← couvre ← module → programme` ;
- l'export Excel au format du client, en V2 avec le SaaS.

Pilote : RS0101 × OF EXEMPLE (`tableau croisé du pilote`). Le
test d'acceptation est de retrouver les écarts de la lecture humaine (C2, C3
et C4 sous la référence, C5 élargi, un chapitre hors référentiel).

### D414 — Flows livrés et ordre de déploiement

Tous les flows sont livrés en brouillon (D144). Ordre de déploiement sur
l'org 4 :

1. **Imports** (pas des flows) : relancer « SIRENE · réseau » puis « DGEFP ·
   réseau », puis « CPF · RS » et « CPF · RNCP ». Leurs récurrences sont
   posées par le pack (lundi à vendredi, au gré de la session pair).
2. **« Quali-Watch · network rules »** (quotidien, 05:30) : ADM-01, ADM-02,
   CPF-01/02/03, REG-01 et REG-02. Il remplace « rule ADM-01 », qui reste
   pour compatibilité ; ne pas déployer les deux.
3. **« Web presence · find websites »** (déjà là) : les sites du réseau.
4. **« Quali-Watch · website check »** (hebdomadaire, dimanche 02:00) :
   `site_crawl` des sites confirmés du réseau, puis COL-01, WEB-01/02/03/04,
   WEB-05, WEB-06 et WEB-08. Il demande les preuves (D408) et finit par
   REG-02 côté pages.
5. **« Quali-Watch · reminders »**, une fois les écarts validés.

## 4. Lots

| Lot | Contenu | Tests |
|---|---|---|
| **R1** | D402 `traverse` + `with_links` ; D403 `per_object.linked` ; D406 avertissement | `partner_watch.rs` (réseau, liens joints, enrichissement borné, siège sans coordonnées) |
| **R2** | Pack `quali_watch` 1.4.0 : `certification.numero`, intitulé 2 000 car. `invalid: skip`, imports SIRENE · réseau, DGEFP, CPF RS / RNCP, alias de nom | import des échantillons réels (DGEFP, CPF RS0101) |
| **R3** | D407 nœud `site_crawl` (cœur, runtime, route interne, garde, NodeDoc, palette, ftl, fiche) | `site_crawl.rs` (serveur de test local) |
| **R4** | D408 preuve serveur par CDP | test sur le navigateur de test (`PNEX_BROWSER_URL`), ignoré sans |
| **R5** | Règles et flows du pack (D409–D410), fiche `assistant-kb` | `pack_rules.rs` (chaque règle sur un cas positif et un négatif) |
| V2 | NOM-01, WEB-07, LIEU-01, CPF-04, PROG-*, `llm_extract`, vision WEB-06, sens « open data → client » de REG-01 | — |

## 5. Questions pour Shan (défaut retenu en attendant)

- **Q-N1 — Navigateur de plateforme pour les preuves** : une org sans
  navigateur à elle peut-elle faire prendre ses preuves par le navigateur du
  cluster (Obscura) ? **Défaut : oui**, comme les règles de site. C'est
  l'opérateur qui l'active (`PNEX_BROWSER_URL`) ; sans lui, la tâche attend
  un agent.
- **Q-N2 — Règles par client** : réglage du pack `regles_inactives` (codes),
  ou objets `regle` éditables par le client (gravité, activation, référence
  légale) ? **Défaut : le réglage** pour le MVP. Les objets `regle` viendront
  avec l'écran Réglages du SaaS.
- **Q-N3 — Organismes non habilités** (CPF-01) : faut-il créer dans
  l'ontologie du client une organisation pour chaque organisme qui vend sa
  certification au CPF sans habilitation (OF HORS RESEAU, OF AUTRE SITE…) ?
  **Défaut : oui**, sans enrichissement SIRENE (hors réseau, D403). Ils
  restent visibles par leurs écarts.
- **Q-N4 — Écarts sur texte** : une règle déterministe qui lit une page
  écrit-elle `a_valider` ? **Défaut : oui** (D409), seules les règles de
  registre écrivent `propose`.
- **Q-N5 — Fréquences** : parcours hebdomadaire (PRD Q5 : mensuel suffit-il
  ?) ; règles réseau quotidiennes. **Défaut : hebdomadaire**, dans le cron du
  flow, que le client change.

## 6. Sécurité (grille `security.md` §6)

- **R1** : l'org vient du nœud déployé, jamais de la requête du runtime ; les
  imports tournent dans l'org de l'import.
- **R2** : aucune route publique nouvelle. Les routes internes passent par le
  jeton du runtime, et la configuration est lue dans la version déployée.
  `with_links` ne lit que l'ontologie de l'org (même service, lecture).
- **R8** : URL du parcours, de la preuve serveur et des imports derrière la
  garde de sortie ; hôtes opendatasoft publics.
- **R11–R13** : HTML de preuve nettoyé et servi en téléchargement seulement,
  capture affichée, texte de page jamais rendu en HTML.
- **Contenu hostile** : le texte des pages est une donnée. Les fonctions
  Starlark n'en tirent que des comparaisons, et le LLM le reçoit délimité
  (D208) ; sa citation est revérifiée.
- **MCP / IAM** : pas d'opération HTTP nouvelle ; les imports et les flows
  restent sous les permissions existantes.

## 7. État (2026-10-24)

- **R1–R2** (`4e15cc91`) : `traverse` et `with_links` du nœud `ontology_query` ;
  `per_object.linked` ; avertissement pour un établissement sans coordonnées ;
  `quali_watch` 1.4.0 (numéro de certification, intitulé de 2 000 caractères
  `invalid: skip`, imports SIRENE et DGEFP du réseau, catalogue CPF RS et
  RNCP). Test : `partner_watch` `network_registers_and_query`.
- **R3–R4** (`1d529add`) : nœud `site_crawl` (palette, inspecteur, NodeDoc,
  codes `site-crawl-*`) ; preuve serveur (`proofs::server_pass`, boucle
  singleton `server-proofs`). Tests : `site_crawl`
  `flow_node_crawls_the_site_of_its_message`,
  `waiting_proofs_are_tried_by_the_server_a_bounded_number_of_times`,
  `server_snapshot_of_a_public_page` (ignoré, navigateur réel : Chrome local,
  PNG pleine page, HTML et texte lus).
- **R5** : fonctions `qw_regles_reseau`, `qw_contexte`, `qw_cibles`,
  `qw_regles_web`, `qw_ecart`, `qw_liens` ; flows « Quali-Watch · network
  rules » et « Quali-Watch · website check » ; réglages `certifications` et
  `regles_inactives` ; fiche `assistant-kb/web-sites.md`. Tests :
  `pack_rules` (règles réseau, cibles, règles des pages, écart puis liens).

Raccourcis notés : un seul site par partenaire (le premier), 200 liens joints
par ligne et par type (une certification à 4 000 partenaires est tronquée),
la règle CPF-01 ne relie pas l'écart à l'organisme (l'offre, oui).
