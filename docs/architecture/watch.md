# Veille — pack générique `watch` (D972–D975)

> Statut : **pack livré (2026-11-14), non commité**. Demande de l'application
> commerciale (dossier entreprise, écran Veille), tranchée par Shan le
> 2026-11-14 : un pack neutre maintenant, tout le reste en configuration
> (flows, pipelines, recettes), aucune nouvelle mécanique.

Liens : `account-mapping.md` (D539 presse GDELT, amendée), `crm.md` (D590
listes de salon, amendée), `pipeline-engine.md` (D934+), `proposals.md`
(D375, liens proposés), `ontology.md` (D190 packs).

## D972 — Un pack neutre : actualités, sources, salons

- Types : `news_item` (clé `ref`, résumé, extrait, date, source, langue,
  crédit, tonalité, `topic` en texte libre, `state` à valider / retenue /
  écartée), `news_source` (adresse unique, format flux ou page), `show`
  (début, fin, lieu, ville, pays, site).
- Liens : `item_about` (actualité → tout objet), `from_source` (actualité →
  source, une seule), `source_of` (source → organisation), `exhibits_at`
  (organisation → salon, attribut `stand`), `organized_by` (salon →
  organisation ; un salon de groupement n'est que cela).
- Le pack ne livre **aucun** thème, secteur, source, salon ni ensemble de
  sociétés suivies. Ce qui compte comme actualité, les sociétés suivies, la
  langue du résumé et le rapprochement des noms sont dans les flows de l'org
  (nœud LLM ou regex). Un rapprochement certain (SIREN, nom exact) lie
  directement ; un rapprochement douteux passe par `link_upsert propose: true`
  (D375), validé par une personne.
- `state` est la décision humaine : un flow peut écarter un homonyme, seule
  une personne retient.

## D973 — La prospection et le CRM s'appuient dessus

- L'import *Presse (GDELT)* (D539) écrit des `news_item` liés par
  `item_about` ; le flow *news to topics* lit `news_item`. `sales_signal`
  garde les signaux des registres (BODACC, établissements, pages suivies) ;
  sa valeur `press` ne reste que pour les signaux anciens.
- `crm` 4.0.0 : `trade_show` disparaît au profit de `show`, que le CRM étend
  de sa seule `list` (listes à transformer en pistes, D590) ; `met_at` et
  `from_list` visent `show`. **Les chiffres d'un salon** (coût, opportunités,
  gagnées, conversion, CA gagné, retour) **et leur flow sont supprimés** :
  ils reviendront si le besoin revient. Bêta : pas de migration.

## D974 — Lecture : la requête d'ontologie, pas de route dédiée

L'application lit `POST /api/v1/ontology/query` :

| Besoin | Requête |
|---|---|
| Actualités sur une organisation | `type_key: organization, ids: [id], traverse: [{item_about, in}], order_by: date, descending` |
| Actualités de ses propres sources | `traverse: [{source_of, in}, {from_source, in}]` |
| Salons où elle expose | `traverse: [{exhibits_at, out}], order_by: start_on` |
| Fil global | `type_key: news_item, filters: [state = retained], order_by: date, descending` |
| Salons à venir | `type_key: show, filters: [start_on >= $today], order_by: start_on` |
| Salons d'un groupement | `ids: [groupement], traverse: [{organized_by, in}]` |

Une route dédiée seulement si la mesure sur 10 000 organisations le demande.
Test : `tests/partner_watch.rs` `the_news_and_shows_of_an_organization_are_one_query_each`.

## D975 — Les hôtes des sources de l'org (Shan, 2026-11-14 : « petite modif du moteur »)

- Un pipeline peut déclarer `object_hosts` : des adresses tenues par les
  objets de l'org (ici `news_source.url`, format `feed`). Un `fetch` peut
  joindre leurs hôtes en plus des `hosts` du document.
- **Jamais de secret vers ces hôtes** : une clé du coffre, un jeton
  obtenu par `token`, ne partent que vers les `hosts` écrits dans le
  document (R9).
- **Garde egress (R8)** : le résolveur filtrant s'applique comme partout ;
  une adresse IP littérale ne passe pas par lui, elle est donc contrôlée
  avant l'appel (`egress::check_url`) ; redirections sur le même hôte
  seulement (D942).
- Pipeline livré : `watch/rss` (RSS 2.0 et Atom), import *Flux d'actualités
  (RSS, Atom)* : une `news_item` par article (clé = adresse, jamais remise à
  zéro), liée à sa source par `from_source`. Registre : SEC-89.
- Limite : un flux mal formé fait échouer le run (raccourci noté dans le
  pipeline).
- Tests : `pipeline_engine::tests::object_hosts_open_their_hosts_but_never_to_a_secret`,
  rejeu de `watch/rss`, `tests/partner_watch.rs`
  `a_feed_of_the_org_becomes_news_items_of_its_company`.
