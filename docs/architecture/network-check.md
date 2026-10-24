# Contrôle d'un réseau de partenaires de bout en bout (D402–D424)

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
| PROG-01/02/04 | Couverture, heures, modalités du programme | tableau croisé sur programmes déposés | calcul (PROG-02, PROG-04), texte puis LLM optionnel (PROG-01) | majeur | **MVP, lot suivant** (D415–D420) ; collecte du programme sur le site : V2 (D413) |

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

### D413 — Navigation et collecte des programmes (anticipé, V2)

Le tableau croisé lui-même est au MVP, sur des programmes **déposés**
(D415–D420, PRD §8 et QW3b). Ce qui reste V2 est la **collecte automatique**
du programme sur le site du partenaire (QW7). Le studio et le nœud
`site_recipe` (D323–D327) sont livrés ; il manque :

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

### D422 — Les imports du réseau suivent le réglage `certifications`

Constat de la recette R1 (org 4, 2026-10-24) : l'org 4 contient tout l'open
data France Compétences. `linked: [habilite_pour, porte]` y retient donc des
dizaines de milliers d'organismes, coupés à 200 par `max_pages`, sans
garantie que le pilote en fasse partie.

`per_object` gagne `setting: { key: <pack>.<réglage>, property }`. Le réglage
est lu comme une liste de codes séparés par des virgules (comme
`setting_list` des fonctions du pack, en majuscules).

- **Sans `linked`** : l'objet lui-même doit porter un de ces codes. C'est le
  cas des certifications du catalogue CPF.
- **Avec `linked`** : l'autre bout du lien doit porter un de ces codes. Ce
  sont les organismes habilités à ces certifications ou qui les portent
  (SIRENE, DGEFP).
- **Réglage vide**, pack non installé ou réglage inconnu : aucun filtre,
  c'est le comportement de D403.

Le pack 1.5.0 pose `setting: { key: quali_watch.certifications, property:
code }` sur les quatre imports du réseau.

### D423 — Les requêtes des flows suivent le même réglage

Les `ontology_query` sur certification des flows « network rules » et
« website check » n'avaient pas de filtre. Elles prenaient les 500
premières certifications par titre, et RS0101 n'était pas garanti d'en
faire partie.

Un filtre `in` accepte désormais la valeur `"$setting.<pack>.<réglage>"`,
résolue par le serveur à chaque requête. Un réglage vide retire le filtre.
Les trois requêtes du pack 1.5.0 filtrent sur
`$setting.quali_watch.certifications`.

### D424 — Les imports d'un pack suivent ses mises à jour

Constat de la recette R1 : `install_from_pack` sautait un import déjà
présent. Les imports France Compétences de l'org 4 n'ont donc reçu ni
`certification.numero` ni le filtre de D422.

Même règle que pour les fonctions et les flows (D368) :

- **Ce que le pack possède** : le mapping (étapes comprises), la source et
  l'URL. Le mode, la récurrence et `max_pages` sont des réglages
  d'exploitation de l'org : jamais remplacés, même par une reprise.
- `object_imports.pack_written` (migration 000052) garde ce que le pack a
  écrit en dernier (`written`), et la version de l'org remplacée
  (`replaced`) pour pouvoir revenir en arrière.
- **À la montée de version** :
  - un import identique à ce que le pack a écrit prend la nouvelle version
    (`version` + 1) ;
  - un import modifié reste tel quel. Il est listé dans `kept`, et
    `differs` dit ce qui change : `source`, `source_url` ou
    `mapping.<clé>`. Le front l'affiche en info-bulle.
- **Reprise** : `take` sur un import met la version du pack et garde les
  réglages d'exploitation. Pas de fusion automatique : la liste de
  `differs` sert d'aperçu.
- **Imports créés avant 000052** (sans `pack_written`) : ceux qui diffèrent
  du pack sont listés comme modifiés, faute de savoir ce que le pack y
  avait écrit. Une reprise les remet dans le circuit normal.

## 4. Lots

| Lot | Contenu | Tests |
|---|---|---|
| **R1** | D402 `traverse` + `with_links` ; D403 `per_object.linked` ; D406 avertissement | `partner_watch.rs` (réseau, liens joints, enrichissement borné, siège sans coordonnées) |
| **R2** | Pack `quali_watch` 1.4.0 : `certification.numero`, intitulé 2 000 car. `invalid: skip`, imports SIRENE · réseau, DGEFP, CPF RS / RNCP, alias de nom | import des échantillons réels (DGEFP, CPF RS0101) |
| **R3** | D407 nœud `site_crawl` (cœur, runtime, route interne, garde, NodeDoc, palette, ftl, fiche) | `site_crawl.rs` (serveur de test local) |
| **R4** | D408 preuve serveur par CDP | test sur le navigateur de test (`PNEX_BROWSER_URL`), ignoré sans |
| **R5** | Règles et flows du pack (D409–D410), fiche `assistant-kb` | `pack_rules.rs` (chaque règle sur un cas positif et un négatif) |
| **R1b** | D422–D423 : imports réseau et requêtes des flows bornés par le réglage `certifications` (pack 1.5.0) | `partner_watch.rs` `network_registers_and_query` |
| **R1c** | D424 : imports d'un pack mis à jour s'ils sont intacts, listés sinon, reprise qui garde mode, récurrence et pages | `partner_watch.rs` `pack_upgrade_follows_untouched_imports` |
| **R6** | D415–D421 (OCR compris) : dépôt des programmes, découpage, rattachements, PROG-01/02/03/04, tableau croisé et export xlsx | `pack_rules.rs` (découpage et règles sur RS0101 × OF EXEMPLE), test du nœud `sheet_write` |
| V2 | NOM-01, WEB-07, LIEU-01, CPF-04, collecte des programmes sur le site, `llm_extract`, vision WEB-06, sens « open data → client » de REG-01 | — |

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

- **D422–D423** (pack 1.5.0) : imports du réseau et requêtes des flows bornés
  par le réglage `certifications`. Test : `partner_watch`
  `network_registers_and_query` (certification hors réglage ignorée,
  réglage vidé).

- **D424** : imports d'un pack mis à jour s'ils sont intacts. Test
  `partner_watch` `pack_upgrade_follows_untouched_imports`.

Écarts relevés en recette R1 (2026-10-24), non traités :

- **Mise à jour d'un pack** : un import livré qui existait déjà n'était
  jamais touché. **Corrigé** par D424.
- **`PUT /ontology/imports/{id}`** sans `recurrence` effaçait la récurrence.
  **Corrigé** : `source_url`, `recurrence` et `max_pages` omis gardent leur
  valeur, `null` les efface (test `partner_watch`
  `import_update_keeps_what_it_omits`).
- **Flow « Ingestion France Compétences »** : encore livré par le pack, donc
  recréé à la mise à jour (flow 16 sur l'org 4). Son retrait relève du lot
  W0 de D359 (`web-presence.md`), pas encore codé. D'ici là, il ne faut pas
  le déployer.

Raccourcis notés : un seul site par partenaire (le premier), 200 liens joints
par ligne et par type (une certification à 4 000 partenaires est tronquée),
la règle CPF-01 ne relie pas l'écart à l'organisme (l'offre, oui).

## 8. Lot suivant du MVP : le tableau croisé sur programmes déposés (D415–D420)

> Ajouté le 2026-10-24 sur la relecture de la session `pnex-quali-watch` :
> le PRD §8 met PROG-01, PROG-02, PROG-04 et le tableau croisé du pilote
> au MVP, à partir de programmes **déposés** (QW3b). Codé **après la
> recette sur dev** de R1–R5, pour ne pas mélanger.

Exemple à reproduire : `tableau croisé du pilote`, une
feuille.

- En titre (`B1:E1` fusionné), l'intitulé de la certification.
- En tête : lien du programme de référence (C2–D2), format de la formation
  (B4–C4), heures de suivi synchrone (B5–C5), heures de préparation à la
  certification (B6–C6).
- Le tableau commence ligne 8, sur 4 colonnes : compétence du référentiel
  (« C1. … »), programme de référence (« Cadrer le projet… (C1) (2h) » et
  son contenu), programme du partenaire (« Chapitre 01 : … »), écarts et
  recommandations.
- Il compte une ligne par compétence C1…C6, puis une dernière ligne
  « Chapitre complémentaire, sans compétence associée » (le chapitre 5 de OF EXEMPLE).

### D415 — Dépôt d'un programme

Un humain dépose le fichier, PDF texte, DOCX ou texte, dans la médiathèque
de l'org. Il crée ensuite l'objet `programme` :

- `role` vaut `reference` ou `partenaire` ;
- `programme_pour` le relie à la certification, `programme_de` à
  l'organisation ;
- la propriété `source` (nouvelle) porte l'id du média.

Il lance enfin l'action **Analyser** (`programme.analyser`, D183), qui
déclenche le flow livré par le pack. Le format, les heures synchrones et les
heures de préparation se saisissent sur l'objet quand le document ne les
donne pas lisiblement. Aucun envoi de fichier par le partenaire au MVP :
le lien de dépôt de la relance R1 viendra avec le SaaS (QW5).

### D416 — Nœud `document_text`

Ce nœud générique lit le texte d'un média `document` de l'org, en dernière
version (`pnex_core::doc_extract` : PDF, DOCX, XLSX, texte).

- **Entrée** : `msg.payload.media_id`.
- **Sortie** : `msg.payload = {media_id, version, text (200 000 car. au
  plus), pages}`.
- **Échec** : média absent ou d'une autre org, type non lisible, PDF sans
  texte (scan). Il faudra 2 codes d'erreur.

Il lit seulement, dans l'org du nœud déployé (R1). Un PDF en colonnes est le
risque connu (D328) : il est à essayer sur le PDF de CERTIF EXEMPLE
avant de figer.

**Sans texte, OCR (Shan, 2026-10-24).** Un PDF scanné ou une image est lu
par un modèle d'OCR de l'org. Ce modèle se charge comme les autres
(registre des modèles, `ml_models`, D81–D83) : fichier ONNX déposé, contrôle
de chargement et d'inférence, inférence en Rust. Il n'y a ni Python
(D204), ni service tiers.

- La tâche `ocr` est nouvelle, à côté de `detection`, avec une première
  famille `paddleocr` : PP-OCR détection + reconnaissance, Apache-2.0, deux
  fichiers ONNX et le dictionnaire des caractères.
- Le texte lu porte `ocr: true`, et chaque extrait cité garde sa page.
- Sans modèle d'OCR dans l'org, le nœud échoue en
  `document-text-no-text`, et le message dit d'ajouter un modèle.
- Licence et poids du modèle : à vérifier avant l'ajout (règle des briques
  permissives).

### D417 — Découpage déterministe en modules

La fonction du pack `qw_decouper` lit le texte et en tire les modules : un
titre suivi de son contenu, jusqu'au titre suivant. Un titre se reconnaît à
un motif d'une liste fermée :

- « Chapitre NN : … » ou « Module N – … » ;
- une ligne qui finit par « (Cn) » ou « (n h) » / « (nh) ».

Pour chaque module, elle relève le rang, le titre, les heures (« (2h) »,
« 2 heures », « 1h30 »), le contenu et les **codes de compétence cités**
(« (C3) », « C3. »). Le résultat va en `module` (`module_de` le programme)
par `object_upsert`. Un nouveau dépôt donne une nouvelle `version` du
programme, et l'ancienne reste.

### D418 — Rattachements `couvre`, du plus sûr au moins sûr

1. **Code cité** dans le titre ou le contenu (« (C1) ») :
   `rapproche_par: code_cite`, `valide: true`. C'est le cas du programme de
   référence de l'exemple.
2. **Intitulé** : les mots du titre du module, normalisés par `fold` et sans
   mots vides, recouvrent au moins 60 % de ceux du libellé de la compétence,
   et le meilleur candidat dépasse le second de 15 points :
   `rapproche_par: intitule`, `confiance`, **`valide: false`**. Cela couvre
   les chapitres de OF EXEMPLE, qui reprennent le libellé sans le code.
3. **LLM** (optionnel, org avec fournisseur) : sur les modules restés sans
   rattachement, avec la liste des compétences. La réponse est un code et une
   citation, et la citation est retrouvée dans le contenu du module (D316) :
   `rapproche_par: llm`, `valide: false`.

Un rattachement `valide: false` est montré « à valider ». L'action
`valider_rattachement` du pack (ontologie §2) le fixe.

### D419 — Règles PROG

Elles sont calculées sur la vue certification → compétence ← `couvre` ←
module → programme, par la fonction `qw_regles_prog` :

| Code | Calcul | État de l'écart |
|---|---|---|
| PROG-01 | compétence sans aucun module du partenaire qui la couvre | `a_valider`, avec la mention « rapprochement par le texte » quand la couverture des autres compétences vient de l'intitulé |
| PROG-02 | heures des modules du partenaire sur la compétence < heures de la référence − `tolerance_heures` (réglage du pack, défaut 0, PRD Q9) | `propose` si tous les rattachements de la ligne sont `valide`, sinon `a_valider` |
| PROG-03 | module du partenaire sans aucune compétence (chapitre hors référentiel) | info, `a_valider` |
| PROG-04 | format, heures synchrones ou heures de préparation du partenaire ≠ référence | `propose` |

Test d'acceptation sur le pilote : retrouver la lecture humaine du PRD §5.7
sans main humaine, rattachements à valider mis à part :

- C2 2 h au lieu de 4, C3 5 h au lieu de 6, C4 3 h au lieu de 4
  (PROG-02) ;
- C5 élargi à 5 h au lieu de 3 (aucune règle, la ligne le montre) ;
- le chapitre 5 hors référentiel (PROG-03) ;
- aucun PROG-01.

### D420 — Export xlsx au format du client : nœud `sheet_write`

Ce nœud générique écrit un classeur xlsx dans la médiathèque de l'org, à
partir de `msg.payload = {name, sheets: [{name, cells: [{ref, value, bold?,
wrap?}], merges: ["B1:E1"], widths: {B: 60}}]}`. C'est un document, avec
une nouvelle version à chaque écriture, et il n'y a pas de formule.

L'écriture se fait **sans nouvelle dépendance** : `zip` et `quick-xml` sont
déjà au cœur (`doc-extract`), et chaque cellule est écrite en texte en ligne
(`inlineStr`).

La fonction du pack `qw_tableau_croise` construit la mise en page de
l'exemple :

- l'en-tête de la référence, puis une ligne par compétence ;
- la colonne « Écarts constatés & recommandations » remplie depuis les
  écarts PROG-* de la ligne ;
- une dernière ligne pour les chapitres hors référentiel.

Le flow « Quali-Watch · cross-table » (action Analyser du programme
partenaire) va du découpage aux rattachements, puis aux règles, aux écarts
et au classeur. Le classeur est relié au programme (`constate_dans`) et
téléchargeable depuis la médiathèque.

### D421 — Les médias d'une page sont des objets reliés à la page (Shan, 2026-10-24)

Un média trouvé dans une page pertinente devient un objet de l'ontologie,
relié à la page, avec sa propre date. Au MVP, ce sont les documents liés
(PDF, DOCX) ; plus tard, les images (logos, WEB-06 par la vision) et les
vidéos.

- Le parcours `site_crawl` relève les liens d'une page pertinente vers un
  document. Il les récupère par le client gardé : même site ou hôtes
  permis, 20 Mo au plus, plancher de 2 s, robots.txt.
- Chacun est gardé en média `document` de l'org, une version par SHA-256
  (D311), et relié à la page par le lien `contient` (nouveau, du pack) :
  `url`, `vu_le`, `premiere_vue`, `sha256`.
- Un document plus lié par la page voit son lien fermé à la date du
  parcours (historique gardé, comme une page disparue).
- Le média garde ses propres dates : la version et quand elle a été vue.
  L'écart qui le cite (`constate_dans` vers `media_asset`, déjà permis) dit
  donc quel fichier et de quand.
- Le programme d'un partenaire peut être pris parmi ces documents :
  - au MVP, un humain le choisit sur la page observée, « en faire le
    programme » ;
  - le choix automatique (titre ou contenu « programme » et code de la
    certification) est en V2 avec D413.

### Questions pour Shan (défaut en attendant)

- **Q-N6 — Programmes scannés** : **tranché (Shan)** : OCR par un modèle de
  l'org chargé comme les autres (D416).
- **Q-N7 — Tolérance des heures** (PRD Q9) : **tranché (Shan)** : tout
  relever (« 4 h prévues, 3 h 30 faites » est un écart PROG-02) ;
  `tolerance_heures` = 0 par défaut, réglable dans le pack. **En best
  effort** : des heures illisibles d'un module ou d'une compétence ne lèvent
  pas PROG-02 et ne font pas échouer le run ; elles donnent un écart
  PROG-00 « heures non lues » (info), pour que l'humain sache que la
  comparaison n'a pas pu se faire. Un rattachement incertain donne un écart
  « à valider », jamais un blocage.
- **Q-N8 — Rattachement par l'intitulé** : **tranché (Shan, confiance)** :
  il compte comme couverture pour PROG-01 et reste « à valider ».
- **Médias d'une page** : **tranché (Shan)**, D421.
