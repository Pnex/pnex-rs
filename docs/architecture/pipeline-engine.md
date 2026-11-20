# Moteur de pipelines unique : toute source et toute analyse en YAML (D934–D953)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-12** (§2). Rien n'est codé. Questions techniques restantes : §9 |
| Déclencheur | Relevé du 2026-11-12 (§1) : l'ingestion des marchés a coûté ≈ 27 300 lignes Rust modifiées pour ≈ 1 660 lignes de YAML ; chaque source nouvelle demande une release |
| Remplace | Les étapes d'import D317–D322 et D344, le document de transformations D571, la recette comme document séparé (D780), les formes de réponse D540, les chargeurs D807–D809 et D889, les routes `/markets/*` et `/mt5/*` (§7) |
| Garde | `data-pipelines.md` (tables versionnées D569, worker D572–D574, ordonnanceur D575, contrôles D576, synchro D577), `connectors.md` (D791–D795), `analysis.md` (D825–D832), `external-apps.md` (D432, D441) |
| Numérotation | **D934…D953**, lots **PE0…PE9** |
| Sécurité | `security.md` §6 avant chaque lot ; §6 de ce document |

## 1. Constat (relevé dans le code, 2026-11-12)

Trois documents qui ne se composent pas :

| Brique | Ce qu'elle fait | Limite |
|---|---|---|
| Pipeline (`pnex-core/src/pipelines.rs`) | Des transformations SQL de table à table, avec contrôles | Aucune étape de source |
| Import (`imports:` d'un pack, `pnex-core/src/ingest.rs`) | Six étapes en ligne puis le mapping vers l'ontologie | Un fichier par passe, pas de boucle, pas d'intermédiaire nommé |
| Règle de navigation (`pnex-core/src/site_recipe.rs`) | `open`, `click`, `extract`, `for_each` | Sa sortie n'alimente pas un import |

Ce qui dépasse ce cadre est écrit en Rust, par fournisseur :

- le mapping porte un champ par source : `press_feed`, `events_feed`, `conflict_feed`, `shape`
  (douze parseurs compilés, `ResponseShape`), `keyed_source` ;
- deux chargeurs à `enum Job` (`services/markets/history.rs`, `services/bulk_history.rs`), chacun
  avec sa file, ses pauses et son registre ;
- sept balayages au lieu d'un ordonnanceur (imports, history, bulk, calendar, documents,
  prediction, identity) ;
- le moteur d'import appelle le métier après écriture (`moves::after_store`,
  `world::confirm_leads`) ;
- les règles de navigation livrées sont des chaînes YAML dans du Rust (`default_rules.rs`) ;
- l'analyse des marchés a ses propres routes (`/markets/*`, 14 chemins) et ses services.

Environ 13 800 lignes Rust sont propres à un fournisseur. La garde D441 ne les voit pas : elle
cherche les clés de pack, pas les noms ni les hôtes des sources. Les tests sont du Rust à mocks
(`tests/markets.rs`, 4 993 lignes) ; un mock partagé a caché une erreur d'hôte.

Les gestes que le moteur ne sait pas déclarer, relevés dans ce code :

| Geste | Exemples |
|---|---|
| Énumérer | codes d'un réglage × périodes ; objets vivants ; pages ; parties numérotées |
| Collecter des liens | dernière ressource d'un catalogue ; liens d'un index ; pièces d'un dépôt |
| Passer par un fichier intermédiaire | page → formulaire à jeton → zip ; jeton → requête ; somme de contrôle voisine ; tar de `json.gz` |
| Tenir un état par élément | fait, vide, à redemander dans N jours, période non close |
| Tenir une cadence | pause entre requêtes, un fichier toutes les cinq minutes |
| Lire | CSV sans en-tête ; binaire à enregistrements fixes sous LZMA ; JSON imbriqué ; tableau HTML ; iCalendar ; fuseau sans heure d'été |

## 2. Choix de Shan (2026-11-12)

| Question | Réponse |
|---|---|
| Combien de types de document | **Un seul** : tout est un pipeline |
| Échappatoire pour un format non couvert | **Une fonction Starlark**, en dernier recours |
| Périmètre | **Tout**, analyse des marchés comprise |
| Données de dev déjà chargées | **Repartir de zéro** |
| Compatibilité | Aucune : l'application n'est pas en production (beta) |
| Pages du front qui lisaient `/markets` et `/mt5` | **Blocs vivants livrés par le pack** ; les terminaux passent à la page des connecteurs |
| Ordre des portages | **Un import simple et une source tordue en témoins**, puis le reste par pack |

> **Borné par D998** (2026-11-18) : « tout » = toute l'**ingestion** et les calculs dérivés
> recalculés depuis leurs sources ; faire et défaire les liens déduits de l'ontologie reste
> à un flow ou à un humain. Écarts et rattrapage : `operational-platform.md`, D998.
| Studio | **Celui des pipelines seul** ; celui des sites web y est replié |
| Texte des dépôts SEC rendu en média indexé (2026-11-13) | **Sixième cible `write: media`** : une colonne de texte devient un média de l'org, indexé et lié à un objet |
| Marchés de prédiction derrière le proxy de l'org (2026-11-13) | **`proxy` générique de `fetch`** : l'adresse du proxy est un réglage de l'org, l'egress s'y applique |
| API des règles de navigation (`/site-recipes`) une fois le studio replié (2026-11-13) | **Supprimée, rupture** : une règle est un pipeline à étape `browse` ; routes, outils MCP, IAM et CLI retirés |
| Pages du front des marchés (2026-11-13) | **Modèles de page du pack** dont les blocs vivants (D831) jouent des requêtes nommées liées à l'objet ou à la date ; les pages Rust partent |
| Clients synchrones de `/markets/*` (2026-11-13) | **Asynchrone partout** : file + `GET /analysis/queries/{id}`, rupture `feat(api)!` |
| Règles de navigation existantes (2026-11-13) | **Migrées en pipelines** à étape `browse` ; la page « Sites web » et son menu partent |

## 3. Le principe en une phrase

Chaque intermédiaire est une **table de travail** : une liste d'adresses, une liste de fichiers
téléchargés, des lignes lues. Énumérer produit des lignes ; aller chercher s'applique à chaque
ligne ; lire transforme des fichiers en lignes ; écrire pose le résultat dans une table ou dans
l'ontologie. Le moteur connaît des transports et des formats, jamais une source.

## 4. Décisions

### D934 — Un seul document : le pipeline

Un pipeline est un document YAML versionné : `key`, `hosts`, `steps`, `schedule`, `tests`. Un
import est un pipeline dont la dernière étape écrit dans l'ontologie ; une règle de navigation est
une étape `browse`. Les documents d'import, de transformations et de recette disparaissent comme
formats séparés. Un pack livre ses pipelines ; une org écrit les siens dans le même format et le
même studio.

> **Bornée par D998** (`operational-platform.md`, 2026-11-18) : « un seul document » vaut
> pour l'**ingestion** (gros volumes, planifiée, ajout et réconciliation). Faire et défaire
> les liens déduits de l'ontologie n'est pas un pipeline : c'est un flow ou un geste humain,
> rejouable sur les données du moment. Une décision de ce genre dans un pipeline demande
> une ligne au tableau des écarts de D998.

### D935 — Tout intermédiaire est une table de travail

Chaque étape lit une table de travail nommée (`from` ou `for_each`) et en écrit une (`as`). Une
table de travail vit le temps du run, dans la zone de transit de l'org, sous son quota. Elle
s'inspecte comme une table (D944). Une colonne `file` porte une clé de fichier, jamais des octets.

### D936 — Neuf étapes fermées

| Étape | Rôle |
|---|---|
| `enumerate` | Produit des lignes : codes d'un réglage, périodes (`year`, `month`, `day`, `hour`), objets d'un type, suite d'entiers, produit croisé de plusieurs |
| `fetch` | Une requête HTTP par ligne (`GET`, ou `POST` de formulaire) ; pagination de D793 ; rend un fichier par ligne |
| `browse` | Les actions de navigation (D323) par ligne, sur le navigateur serveur ou un agent (D782) |
| `extract` | Tire des lignes d'un fichier : liens, champs d'un formulaire, tableau HTML, pointeurs JSON, expression régulière |
| `unpack` | Ouvre une archive ou une compression : zip, tar, gzip, lzma ; une ligne par entrée retenue |
| `read` | Fichiers → lignes typées, par un lecteur de format (D939) |
| `sql` | Une requête DataFusion sur des tables de travail et des tables de l'org (D572 inchangé) |
| `call` | Une fonction Starlark de l'org ou du pack (D940) |
| `write` | Pose le résultat dans une cible (D941) |

Les étapes de D317–D321 s'y rangent : `source_resolve` = `fetch` + `extract`, `file_download` =
`fetch`, `archive_extract` = `unpack`, `table_inspect` = la sonde, `rows_filter` = `sql`,
`connector_query` = `fetch` ou `sql` sur un connecteur.

### D937 — État par élément, un seul registre

Une étape `enumerate` peut déclarer `track: true` : chaque ligne a alors un état gardé entre les
runs, par pipeline et par clé de ligne : `done`, `empty`, `failed`, avec `retry_after`. Une étape
aval déclare ce que vaut une réponse vide (`empty: settled`, ou `empty: retry` avec un délai) et
`closed_only` écarte une période non terminée. Un run prend d'abord les lignes jamais demandées,
puis celles à redemander. Ce registre remplace `market_history_partitions`.

### D1000 — Lire une fois, ou relire tous les N jours (2026-11-18)

Une ligne `done` n'est jamais reprise : c'est la **collecte unique** (une adresse déjà lue ne se
relit pas). L'étape suivie peut déclarer `again_days: N` : une ligne `done` redevient à prendre N
jours après sa lecture, c'est la **collecte périodique** (même adresse ; l'écriture `objects`
retrouve l'objet par la clé de son mapping et le met à jour, jamais de doublon). Les lignes jamais lues passent toujours avant. `again_days` exige `track: true`,
vaut au moins 1. Choix de Shan : le plus générique et le plus simple à lire, un seul mot.

### D938 — Cadence par hôte

`fetch` et `browse` portent `every` (un appel par durée) et `pause`. La cadence se compte **par
hôte, pour toute la plateforme** : deux orgs qui lisent la même source ne doublent pas le débit.
`429` et `5xx` sont relancés avec attente (`Retry-After` respecté), comme en D793.

> **Précisée le 2026-11-15 (pipeline `markets/dukascopy_m1`)** : un `429`, un `503` ou une
> connexion coupée sur une ligne suivie **arrête la passe** ; la ligne n'est pas marquée (son
> compteur d'échecs ne monte pas), les suivantes attendent la passe d'après. Un hôte cadencé
> (`every`) qui repousse est mis en pause : une minute, doublée à chaque refus d'affilée, une heure
> au plus ; une réponse lève la pause. Pendant la pause, la requête est refusée sans partir
> (`throttled`). Tenu par processus, sur `get` et les formulaires seulement (pas les flux).

> **Amendée par D975 (2026-11-14, `watch.md`)** : un document peut aussi déclarer
> `object_hosts`, des adresses tenues par les objets de l'org, dont les hôtes deviennent
> joignables ; jamais avec un secret, et une IP littérale passe la garde egress.

### D939 — Des lecteurs de format, jamais de fournisseur

`read` accepte : `csv` (séparateur, en-tête ou colonnes par position, encodage), `json` (pointeur
des enregistrements, dépliage d'un tableau imbriqué), `xml` (chemin), `html` (page web lue
comme un document : `/ld` les objets JSON-LD, `@graph` déplié ; `/meta` les balises `<meta>`, le
titre et l'adresse canonique ; `/links` chaque lien et son texte ; puis pointeurs comme en JSON), `ical`, `lines`, `binary` (enregistrements de taille fixe : champs, type, boutisme).
Une colonne déclare son type, son format de date, son fuseau, son échelle décimale. Le fuseau est
un nom IANA (`America/New_York`), heure d'été comprise ; un décalage fixe (`-05:00`) seulement pour
une source qui ne change jamais d'heure. Relevé du 2026-11-12 : les fichiers HistData sont à l'heure
de New York **avec** heure d'été, et l'ancien chargeur ajoutait 5 h fixes, ce qui mettait toutes ses
minutes et ses ticks 1 h en retard d'avril à octobre (le chiffre de l'emploi américain de 08:30 NY à
13:30 UTC au lieu de 12:30). Le test du pipeline HistData vérifie qu'un vendredi d'été finit à
20:59 UTC et qu'une minute de juillet à 08:30 NY tombe à 12:30 UTC.

Les conversions de fuseau se font **dans le pipeline seulement** (Shan, 2026-11-12) : à la lecture par
le `timezone` d'une colonne, ou dans une étape `sql` par deux fonctions du worker,
`local_to_utc(ts, zone)` (heure murale → instant UTC) et `utc_to_local(ts, zone)`. Une heure vécue
deux fois à l'automne est lue comme la première ; une heure sautée au printemps, avec le décalage
d'avant. Une source décalée d'une valeur fixe toute l'année se corrige par
`local_to_utc(ts - INTERVAL '5 hours', 'America/New_York')`. **Codé le 2026-11-12**
(`pnex-pipeline-worker/src/tz.rs`, tests NY sous les deux régimes d'heure d'été). Ajouter un
format est un changement du moteur ; ajouter une source ne l'est jamais.

### D940 — Starlark en dernier recours

`call` passe un fichier ou des lignes à une fonction Starlark (registre des fonctions, D497) et
reçoit des lignes. Elle s'exécute dans le worker : ni réseau, ni secret, ni fichier, mémoire et
durée plafonnées. La validation d'un pipeline signale chaque `call` : c'est une dette de format à
remonter en lecteur quand elle revient deux fois.

### D941 — Cinq cibles d'écriture

| Cible | Effet |
|---|---|
| `table` | Une transaction d'une table versionnée (D569), avec clé et contrôles (D576) |
| `objects` | La synchro ontologie (D577) : objets et liens du mapping D301 |
| `series` | Des valeurs datées d'un objet, avec leur date de connaissance |
| `time_ranges` | Des plages du calendrier de l'org (D182) |
| `proposals` | Des propositions à valider (D371), jamais une écriture directe |

Le mapping garde `objects`, `links` et les conversions D301 ; ses champs par source
(`shape`, `press_feed`, `events_feed`, `conflict_feed`, `keyed_source`, `per_setting_code`,
`rows_in_setting`) sont supprimés.

### D942 — Hôtes déclarés, secrets par connecteur

Un pipeline liste ses `hosts`. Chaque adresse appelée passe l'egress (R8) et doit porter un hôte
de la liste ; une valeur venue d'une ligne ne peut pas changer l'hôte ; les redirections ne sont
pas suivies hors liste. Une source à clé passe par un connecteur `rest` (D793) : le secret reste au
coffre, le pipeline ne nomme que le connecteur. Un contact à déclarer dans le `User-Agent` est un
réglage du pack, lu par `fetch`.

### D943 — Deux zones de confiance, inchangées

Le serveur va chercher (`enumerate`, `fetch`, `browse`, `write`) : il a l'egress, le coffre et
l'ontologie. Le worker lit et calcule (`unpack`, `extract`, `read`, `sql`, `call`) : il n'a ni
secret ni table métier (D573). Un run alterne les deux par la file existante.

### D944 — Une sonde sur toute étape

`inspect` joue un pipeline jusqu'à une étape, sur un élément, sans rien écrire, et rend : format
deviné, encodage, séparateur, colonnes, taux de remplissage, exemples, quelques lignes (le
`describe` de D320, étendu aux fichiers binaires et aux pages). C'est par elle que le studio et
l'assistant écrivent un `read` sans deviner.

### D945 — Tests en YAML

Un pipeline porte `tests:` : des réponses enregistrées par adresse (`given`), et ce qui doit en
sortir (`expect` : colonnes, nombre de lignes, quelques valeurs, état des éléments). Un seul test
Rust joue les tests de tous les pipelines livrés contre un serveur de rejeu, sans réseau. Un
pipeline livré sans test échoue. Les fichiers d'exemple sont plafonnés en taille (§9, Q-PE1).

### D946 — Un ordonnanceur

L'ordonnanceur à dépendances (D575) lance tous les pipelines. Les balayages par domaine
disparaissent. Ce qui suivait un import par un appel en dur devient une dépendance : un pipeline
qui lit une table tourne après celui qui l'écrit.

### D947 — Un pack est un dossier

`packs/<clé>/pack.yaml` (types, liens, réglages), `pipelines/*.yaml`, `queries/*.yaml`,
`rules/*.yaml`, `functions/*.star`, `fixtures/`. Les règles de navigation livrées quittent
`default_rules.rs` pour leur pack.

### D948 — Requêtes nommées

Un pack déclare des requêtes : un `SELECT` à paramètres typés, sur des tables et des
`objects.<type>`, avec `as_of`. Une route générique les exécute sous les droits de l'appelant
(moteur de D825, garde-fous de D826). Elles remplacent les routes de lecture `/markets/*`
(barres, séance, graphique, événements, entrées d'un modèle, scores).

### D949 — Méthodes d'analyse génériques

Deux calculs quittent le domaine des marchés pour `/analysis`, sans nom de pack :

- **mesure autour d'événements** : une série, une liste d'instants, une fenêtre, la même mesure
  sur des instants tirés ; c'est l'étude d'événement de D532 ;
- **chaînes candidates** : le moteur `rca` existant sur les signaux et les `causal_pairs` qu'un
  pack déclare (D904).

La détection de séances anormales devient un `sql` (fenêtres glissantes) suivi d'un `write` ; la
note de séance devient une règle (D829) qui lance une compétence d'agent.

### D950 — Sources poussées

Un terminal ou un modèle externe qui envoie des lignes est un connecteur `webhook` (D795) vers une
table ; un pack peut livrer le fichier à installer côté source comme gabarit. Les routes `/mt5/*`
et `POST /markets/decisions` disparaissent.

### D951 — La garde voit les fournisseurs

`engine_genericity` refuse en plus, dans le code non test : tout hôte public en littéral, et tout
nom de source cité par un pipeline livré. L'existant est gelé dans une liste de dette qui doit
être vide à la fin de PE9.

### D952 — Remise à zéro

Les tables chargées par les anciens chargeurs, leur registre et les imports installés sont
supprimés par migration ; tout se recharge par les nouveaux pipelines. La suppression est totale,
pas une mise à jour : les minutes et ticks HistData déjà chargés sont faux d'une heure en été (D939),
et un rechargement par-dessus laisserait des lignes fantômes (vendredi 21:00–22:00 UTC en été). Les routes retirées sont une
rupture de contrat (`feat(api)!`, `CONTRACT` incrémenté), front, `e2e/`, MCP, IAM et CLI adaptés
dans le même commit.

### D953 — Décisions remplacées

D317–D322 et D344 (étapes et import étendu), D571 pour la forme du document, D540 (formes), D780
pour « un format de règle séparé », D807–D809 et D889 (chargeurs), D847–D849 pour les routes MT5.
Leur texte reste, avec un renvoi ici.

## 5. Deux exemples

Une source à formulaire, par paire et par année :

```yaml
key: histdata_m1
hosts: [www.histdata.com]
steps:
  - enumerate:
      as: parts
      track: true
      cross:
        - { setting: markets.histdata_pairs, as: pair }
        - { periods: year, from_setting: markets.histdata_from, closed_only: true, as: year }
  - fetch:   { for_each: parts, url: "https://www.histdata.com/…/{{pair}}/{{year}}", every: 5m, as: page }
  - extract: { from: page, form: "#file_down", as: form }
  - fetch:   { for_each: form, method: post, url: "https://www.histdata.com/get.php", form: "{{form}}", empty: settled, as: zip }
  - unpack:  { from: zip, entries: ["*.csv"], as: csv }
  - read:
      from: csv
      format: csv
      header: false
      delimiter: ";"
      columns:
        - { name: ts, type: timestamp, format: "%Y%m%d %H%M%S", timezone: America/New_York }
        - { name: open, type: decimal, scale: 8 }
      as: bars
  - write: { from: bars, table: { name: histdata_m1 } }
tests:
  - given:
      "https://www.histdata.com/…/XAUUSD/2015": fixtures/page.html
      "https://www.histdata.com/get.php": fixtures/2015.zip
    expect: { columns: [pair, ts, open, high, low, close], rows: 3 }
```

Un zip, plusieurs tables, puis l'ontologie (ce que cinq imports redéclaraient) :

```yaml
key: registry_export
hosts: [www.data.gouv.fr, static.data.gouv.fr]
steps:
  - fetch:   { url: "https://www.data.gouv.fr/api/1/datasets/{{setting.dataset}}/", as: catalogue }
  - extract: { from: catalogue, json: "/resources", where: { title: "export-fiches-csv-*.zip" }, latest_by: last_modified, as: link }
  - fetch:   { for_each: link, url: "{{url}}", skip_if_unchanged: true, as: zip }
  - unpack:  { from: zip, entries: ["*Certificateurs*.csv", "*Standard*.csv"], as: files }
  - read:    { from: files, entry: "*Certificateurs*.csv", format: csv, delimiter: ";", as: certifiers }
  - read:    { from: files, entry: "*Standard*.csv", format: csv, delimiter: ";", as: sheets }
  - write:   { from: certifiers, objects: { … } }
  - write:   { from: sheets, objects: { … } }
```

## 6. Sécurité (grille `security.md` §6)

| Point | Tenue |
|---|---|
| Org (R1) | Celle du pipeline, jamais d'une ligne ni d'un paramètre |
| Permissions (R2) | `pipelines:*` existantes ; une requête nommée se joue sous les droits de l'appelant ; test 403 par opération |
| Sortant (R8) | Hôtes déclarés (D942), egress par adresse, pas de redirection hors liste, adresse privée refusée |
| Secrets (R4, R16) | Seulement par connecteur ; jamais dans un document, une table de travail, un journal ni une sonde |
| Entrées hostiles | Plafonds existants (octets, entrées de zip, profondeur XML) étendus aux nouveaux lecteurs ; `binary` et `lzma` bornés ; `for_each` plafonné |
| Code utilisateur (R5, R6) | Starlark dans le worker, sans réseau ni environnement ; SQL en lecture, liste blanche de fonctions |
| HTML (R11–R13) | Une page lue n'est jamais rendue sur l'origine de l'application ; la sonde rend du texte |
| Navigation | Règles de D254–D258 inchangées : pas de connexion, pas de formulaire hors champ de recherche (D789). `fetch` en `POST` de formulaire ne renvoie que les champs lus sur la page du même hôte |
| Écriture | `proposals` pour ce qui n'est pas déterministe ; aucune action physique |

Findings à ouvrir au registre §7 en route : les hôtes en littéral du moteur (dette de D951).

## 7. Ce qui est supprimé

| Zone | Fichiers | Remplacé par |
|---|---|---|
| Formes et sources à clé | `pnex-core/src/markets.rs` (`ResponseShape`, `KEYED_SOURCES`) | `read`, connecteurs |
| Flux par fournisseur | `gkg.rs`, `gdelt_events.rs`, `conflict_events.rs`, champs `*_feed` du mapping | pipelines des packs |
| Chargeurs | `markets/history.rs` (core et backend), `bulk.rs`, `bulk_history.rs` | pipelines, registre D937 |
| Étapes | `ingest.rs`, `services/ingest/*` | étapes D936 |
| Balayages | history, bulk, calendar, documents, prediction, identity | ordonnanceur D946 |
| Marchés | `services/markets/*`, `pnex-core/src/markets/*`, `controllers/markets.rs`, `market_decisions.rs`, `mt5.rs` | requêtes nommées, `/analysis`, connecteur poussé |
| Règles livrées | `default_rules.rs` | `rules/` des packs |
| Tests | `tests/markets.rs` et les mocks de source | `tests:` des pipelines |

## 8. Lots

Chaque lot laisse `main` vert et supprime le Rust qu'il remplace dans le même commit.

| Lot | Contenu | Portage témoin |
|---|---|---|
| **PE0** | Garde D951 avec sa liste de dette ; pack en dossier (D947) | — |
| **PE1** | Document, validation, tables de travail, registre D937 (core + migration) | — |
| **PE2** | `enumerate`, `fetch` (GET, formulaire, pagination, cadence), `extract`, `unpack` zip, `read` csv/json, `write table`, sonde, rejeu des tests | Les deux témoins : séries d'une banque centrale (SDMX CSV) et la source à formulaire par paire et par année |
| **PE3** | Connecteurs, sommes de contrôle ; lecteurs `html`, `xml`, `ical`, `binary`, `lines` ; tar, gzip, lzma ; `call` | Ticks binaires ; fichiers de positions ; calendriers |
| **PE4** | `write` vers `objects`, `series`, `time_ranges`, `proposals` ; portage de tous les `imports:` des packs ; suppression de `ingest`, des formes et des champs `*_feed` | Registre de certifications (un zip, cinq tables) ; dépôts et chiffres publiés |
| **PE5** | `browse` ; règles de navigation migrées ; `default_rules.rs` supprimé ; studio des sites web replié dans celui des pipelines | Capture de profil ; recherche d'une personne |
| **PE6** | Ordonnanceur unique ; suites d'import en dépendances ; six balayages supprimés | Séances anormales en `sql` |
| **PE7** | Requêtes nommées (D948) et route générique ; routes de lecture `/markets/*` supprimées ; pages du front (§9, Q-PE1) | Graphique, séance, barres à une date |
| **PE8** | `/analysis` : mesure autour d'événements, chaînes candidates ; sources poussées (D950) ; note de séance en règle | Étude d'événement ; terminal poussé |
| **PE9** | Suppression du reste de §7 ; dette de la garde à zéro ; migration D952 ; fiches de l'assistant, lexique, OpenAPI, MCP, IAM, CLI | — |

À chaque lot : contrat d'API, registres MCP et IAM, fiches `assistant-kb`, clés `.ftl`, lexique
(`design-system.md` §7 : Étape, Table de travail, Sonde, Requête nommée) dans le même commit.

## 9. Questions ouvertes (techniques)

| | Question | Proposition |
|---|---|---|
| Q-PE1 | Taille des fichiers d'exemple des tests | 64 Ko par fichier, 2 Mo par pack |
| Q-PE2 | Un pipeline d'une org peut-il appeler `browse` avec une session d'agent | Oui, aux conditions de D782, sans changement |

## 10. Non vérifié

- Que `read` + `sql` suffisent pour les JSON très imbriqués (chiffres publiés, flux d'événements
  agrégés) sans `call`.
- Le coût de brancher `browse` sur le navigateur serveur depuis l'ordonnanceur.
- Le débit de `write objects` quand la table de travail vient du worker (plafond `PLANNED_MAX`
  de la synchro).
- La mesure autour d'événements en SQL seul : le tirage d'instants demande une graine stable.

## 11. Avancement

### 2026-11-12 — PE0, PE1, premier témoin de PE2

- **PE0** : la garde `engine_genericity` refuse un hôte de source en dur ; l'existant est gelé
  dans `HOST_DEBT` (fichier, hôtes, lot qui l'enlève) et une ligne disparue du code doit quitter la
  liste.
- **PE1** : le document (`pnex-core/src/pipeline/`) — étapes, validation, périodes, gabarits avec
  filtres `lower`/`upper`, état par élément (D937), fuseaux IANA (feature `tz`) ; registre
  `pipeline_items` (migration 000148) ; pipelines livrés dans `packs/<pack>/pipelines/*.yaml`,
  fixtures dans `packs/<pack>/fixtures/` (64 Ko au plus).
- **PE2, premier témoin** : l'exécuteur (`pnex-backend/src/services/pipeline_engine/`) joue
  `enumerate`, `fetch` (GET et formulaire), `extract` (formulaire, liens, JSON, regex), `unpack`
  (zip, tar, gzip, lzma), `read` (CSV, JSON), `write` vers une table ; ordonnanceur une fois par
  minute. **HistData 1 minute est un pipeline YAML** (`markets/histdata_m1`, table `histdata_m1`)
  et l'ancien chargeur Rust de ces minutes est supprimé. Test de rejeu YAML + test d'intégration
  sur base (registre, table, années vides attendues 7 jours).
- **Fuseaux** : `local_to_utc` / `utc_to_local` dans le SQL du worker, `timezone` IANA dans `read`.

Écarts assumés, à reprendre :

| Écart | Où il se lève |
|---|---|
| `unpack` et `read` tournent dans le serveur (tâche bloquante), pas dans le worker (D943) | PE3, avec les gros fichiers |
| Corps tenus en mémoire (512 Mo au plus) et fichier lu par `read` plafonné à 64 Mo | PE3 (ticks d'un mois, AIS) |
| Cadence par hôte tenue par processus, pas par plateforme (D938) | dès qu'il y a plusieurs pods de pipelines |
| Un seul `enumerate` suivi, en première étape | quand une source demande deux niveaux d'état |
| Champs retirés jusqu'à leur code : connecteur, pagination, somme de contrôle, clé et mode de table | PE3 |
| `sql`, `call`, `browse` et `write objects` refusés à l'exécution (`step_unsupported`) | PE3, PE4, PE5 |
| HistData : l'année en cours n'arrive qu'une fois close (l'ancien chargeur la prenait par mois clos) | un `from` relatif (« début de l'année ») dans `enumerate` |
| L'ancienne table `histdata_ohlcv_1m` reste dans les orgs qui l'avaient (heures fausses en été) : à supprimer depuis la page des tables | PE9 (D952) |
| La sonde (D944) n'a pas encore de route | PE2, suite |
| Le second témoin (séries BRI) reste un import | PE2, suite |

### 2026-11-12 (suite) — PE3, première moitié

- **Gros fichiers** : un corps téléchargé et une entrée d'archive passent sur disque au-delà de
  4 Mo (`blob.rs`) ; une lecture CSV est un flux ; les lignes typées d'un `read` vont dans un
  fichier Arrow IPC, relu lot par lot par l'écriture. Plus aucun fichier entier en mémoire, sauf
  une page, un JSON ou un `.bi5` (64 Mo au plus).
- **Ajouts génériques** au document : somme de contrôle publiée à côté (`checksum_url`),
  `header: auto`, horodatage `epoch` à unité lue au nombre de chiffres, `map` de valeurs,
  `null_if`, `range`, `where` et `skip_invalid` sur une lecture, valeur de colonne par gabarit
  de la ligne (`{{instrument}}`, remplace `carry`), énumération imbriquée (`for_each`, `to`),
  colonnes dérivées par motif (`derive`), départs relatifs (`-12m`, `this_month`), filtre `int`,
  lecteur binaire à enregistrements fixes (`divide`, `base`), drapeau `large` (seulement pour
  une org dont un administrateur a fixé le quota, SEC-82).
- **Portés en YAML, Rust supprimé** : klines et transactions Binance, ticks HistData, ticks
  Dukascopy (pack `markets`), AIS danois et américain (`trade_flows`), DVF (`real_estate`).
  `markets/history.rs` ne garde que la passe MT5 (PE8) ; `bulk_history.rs` que ADS-B (porté le
  2026-11-13, voir plus bas).
- **Nouvelles tables, rechargées de zéro** (D952) : `binance_klines_1m`, `binance_trades`,
  `histdata_quotes`, `dukascopy_quotes`, `ais_dk`, `ais_noaa`, `dvf`. Les anciennes restent dans
  les orgs qui les avaient, à supprimer depuis la page des tables.

| Écart | Où il se lève |
|---|---|
| ADS-B : un jour = jusqu'à 26 parties de 2 Go concaténées, à lire en flux sans les garder ; le spool sur disque de `fetch` ne convient pas | une étape `fetch` en flux (tube vers `unpack` et `read`), puis ADS-B en YAML |
| Binance : le mois en cours n'arrive qu'une fois clos (l'ancien chargeur le prenait jour par jour) | `write` avec clé de remplacement, pour lire un jour puis le mois sans doublon |
| NOAA : un jour absent était attendu avant les suivants ; il est maintenant redemandé chaque semaine, les suivants lus quand ils paraissent | à garder tel quel sauf besoin |
| Le contact déclaré (`source_contact`) n'est plus envoyé dans le `User-Agent` des sources de gros fichiers | réglage `contact` du document si une source le demande |

### 2026-11-12 (suite) — PE4 commencé : imports nourris par un pipeline

- **Un import désigne son pipeline** (`mapping.pipeline: <pack>/<clé>`) : le run remplit la table
  de l'import par le moteur (écriture `replace: true`, une transaction instantané), puis la synchro
  existante (D577) écrit objets, liens et valeurs datées. Runs, aperçus, diffs et installation de
  pack inchangés ; pas de changement de contrat d'API.
- **Étape `sql`** : file `pipeline_step_jobs` (migration 000149). Le serveur dépose les tables de
  travail citées en Parquet, le worker exécute la requête avec la session, la liste blanche et les
  limites des transformations (D572), et rend un fichier ; les tests de rejeu jouent la même
  fonction du worker dans le processus. Une requête sur une table vide rend une table vide.
- **Secrets** : un pipeline livré par un pack peut nommer une clé du coffre (`secret: {name,
  param|header, scheme}`), envoyée à ses seuls hôtes ; un document écrit par une org n'en nommera
  jamais (connecteur). En-têtes à gabarit (`User-Agent` avec le contact déclaré).
- **Portés** (13 imports) : BRI, COT, BCE, Eurostat, FRED ×2, ALFRED ×2, EIA, PortWatch, Tiingo ×3.
  **Supprimés** : 7 formes de `ResponseShape` (`DatedCsv`, `DailyPrices`, `EcbCsv`,
  `AlfredVintages`, `PortwatchDaily`, `EiaSeries`, `CftcCot`), `KEYED_SOURCES` et le champ
  `keyed_source` du mapping.
- **Ajouts génériques** : filtres `before:`/`after:` des gabarits, `split` et `exclude` d'un axe,
  `objects` dans les cas de test.

| Écart | Où il se lève |
|---|---|
| Lecture incrémentale (`{since}`) abandonnée : chaque run relit l'historique entier (Tiingo, PortWatch 2000 derniers jours) ; la synchro ne garde que ce qui change | un curseur par code (`track` sur l'axe des codes) si un débit le demande |
| Restent en Rust : SEC (`sec_tickers`, `edgar_filings`, `xbrl_facts`), ESEF, Open Prices, flux GDELT/UCDP/ACLED, imports `per_object` (registres, GLEIF, Wikidata, BODACC, CPF) et les étapes France Compétences | suite de PE4 |
- **2026-11-12 (fin d'après-midi)** : SEC (sociétés, dépôts, chiffres XBRL), ESEF et Open Prices
  portés ; `ResponseShape` supprimé en entier (`markets.rs` : 1 057 → 167 lignes). Lecture JSON
  générique : jokers `*` dans `records`, `keys` (clés traversées), `columnar` (tableaux en
  colonnes), `^/…` (champ de la racine). Un réglage vide compte comme absent ; un en-tête qu'il ne
  remplit pas rend `import-contact-required`.

#### Reste de PE4 : à trancher

| Famille | Difficulté | Piste |
|---|---|---|
| Registres (SIRENE, établissements, BODACC), GLEIF ×3, Wikidata ×2, CPF ×2, DGEFP — `per_object` | les mappings lisent du JSON imbriqué (`each:`, `a.b`, `^parent`) : une table à colonnes plates les perd | une colonne `json` (texte JSON relu en valeur par la synchro), le mapping inchangé |
| France Compétences ×4 (CSV), RS (XML) | étapes `steps` déjà déclaratives ; XML imbriqué pour RS | pipelines CSV tout de suite ; lecteur XML + colonne `json` pour RS ; puis suppression de `ingest` |
| GDELT presse et événements, UCDP ×3, ACLED | tranches de 15 min listées par un index, agrégation par pays et famille, jeton OAuth obtenu au vol (ACLED) | `extract` regex de l'index + `track` des tranches + `sql` d'agrégation ; le jeton : étape `fetch` qui garde sa réponse hors des tables de travail |
- **2026-11-12 (soir)** : **journal des étapes** (D944) — chaque étape d'un pipeline laisse une
  ligne (lignes, octets, première adresse, code d'erreur) dans l'historique du run de son import
  (`ingest_steps`, même vue dans l'UI, libellés fr/en des nouvelles étapes). **France
  Compétences ×4** (certificateurs, fiches, partenaires, blocs) passés en pipelines : colonnes aux
  noms de la source (un fichier déposé à la main se lit pareil), axe d'objets filtré (`only`), table
  vide gardant ses colonnes dans une étape `sql` (un réglage vide garde toutes les lignes, D425),
  entrée d'archive demandée et absente = échec (`ingest-entry-missing`), colonne disparue =
  `import-schema-changed` nommant la colonne de la source. Le garde-fou des fermetures massives
  (`ingest-suspicious-closures`) tient toujours. Le moteur d'étapes `ingest` reste pour le seul
  import RS (XML imbriqué).
- **2026-11-12 (nuit)** : **imports par objet portés** (14) : registre SIRENE ×2, GLEIF ×4,
  Wikidata ×2, établissements, BODACC, liste de ciblage, DGEFP, CPF ×2 (packs `companies_fr`,
  `prospecting_b2b_fr`, `quali_watch`). Ajouts génériques : colonne **`json`** d'une lecture (la
  valeur gardée en texte JSON, `at: "/"` = l'enregistrement entier) ; un import la désigne par
  `json_column: <colonne>` et la synchro la relit en enregistrements (objet, ou chaque objet d'un
  tableau, à côté des autres colonnes) : `each:`, `a.b` et `^parent` des mappings inchangés.
  **`pages`** de `fetch` (numéro de page en paramètre jusqu'à une page sans enregistrement,
  `max` au plus). Axe d'objets : **`linked`** (lien ouvert d'un de ces types, dans un sens ou
  l'autre), **`codes`** (codes d'un réglage comparés à une propriété, de l'objet ou de l'autre
  bout du lien ; réglage vide = aucun filtre), **`title`** (colonne recevant le titre : recherche
  par nom, le SIREN vérifié ensuite en `sql`). **Supprimés** : `per_object`, `per_setting_code`,
  `rows_in_setting`, `contact_setting` du mapping, `fetch_per_object`, le curseur
  `per_object_cursor` (la colonne reste en base jusqu'à PE9).

| Écart | Où il se lève |
|---|---|
| Un run interroge tous les objets visés (l'ancien import en prenait `max_pages` par run, à tour de rôle) : 150 ms par objet, une org de 5 000 sociétés = 12 min par run | `track` + `per_run` sur l'axe d'objets si un débit le demande |
| Une réponse en erreur (429, 5xx) fait échouer le run ; l'ancien import la comptait comme « aucun enregistrement » sauf si toutes échouaient | un `on_error: empty` de `fetch` si une source le justifie |
| Le contrôle de forme d'un import à colonne `json` ne vérifie que la colonne ; un champ imbriqué disparu donne des objets incomplets, pas `import-schema-changed` | contrôle sur les premiers enregistrements relus |
| Restent hors moteur : RS (XML), GDELT presse et événements, UCDP ×3, ACLED, ADS-B, MT5 | suite de PE4, PE8 |
- **2026-11-12 (nuit, suite)** : **fiches RS (XML)** en pipeline (`quali_watch/fc_rs_xml`) :
  `read` lit le XML (`records: FICHES/FICHE`, même lecteur durci que les imports : pas de DTD,
  profondeur bornée), chaque fiche gardée entière en colonne `json`, seules celles des
  certifications de l'org écrites. Le champ du mapping est **`json_column`** (pas `rows`, qui
  reste le chemin d'un fichier envoyé à la main : la fiche RS déposée en XML se lit toujours).
  Plus aucun pack n'utilise les étapes `ingest` (`source_resolve`, `file_download`,
  `archive_extract`, `table_inspect`, `rows_filter`) : seuls les imports écrits par une org
  peuvent encore les porter (repli dans le studio des pipelines, PE5, suppression PE9).

| Écart | Où il se lève |
|---|---|
| Le XML est lu en mémoire (64 Mo au plus, comme un JSON) : un export RS plus gros échouerait (`unreadable`) | lecteur XML en flux, enregistrement par enregistrement |
- **2026-11-12 (nuit, fin)** : **flux d'événements GDELT** (`world_events/gdelt_events`, 48 h, et
  `gdelt_events_history`, 92 jours, lancé à la main) en pipelines : tranches de 15 min par
  `enumerate` heures × quarts, table FIPS → ISO en données (`map` de colonne, avec la nouvelle
  valeur par défaut `"*"`), une piste par (pays, famille, jour) en `sql` (famille la plus longue,
  lieu et région majoritaires, cinq acteurs, 50 identifiants). Ajouts génériques : `requires`
  du document (réglages qui doivent être remplis, sinon le run ne lit rien) et `string_agg` dans
  la liste blanche du worker (valeurs d'un groupe, bornées par les lignes lues). Supprimés :
  `events_feed` du mapping, `fetch_events`, `pnex-core/src/gdelt_events.rs` (591 lignes). La
  confirmation des pistes par un événement vérifié suit tout import qui écrit des `geo_event`.
- **2026-11-12 (nuit, fin bis)** : **événements vérifiés** en pipelines : UCDP GED et Candidate
  (téléchargements), UCDP API (jeton de l'org, pages comptées depuis 0), ACLED (compte de
  l'org). Tables Gleditsch-Ward et ISO numérique → ISO alpha-2 en données (`map`, `derive`).
  Ajouts génériques : `pages.from`, `run_date` / `run_now` dans les gabarits et les valeurs de
  colonnes, filtre `days_before:N`, **jeton obtenu par le `secret` d'un `fetch`**
  (`secret.token` : formulaire OAuth2 « password » vers une adresse d'un hôte du document, avec
  deux secrets du coffre ; le jeton va dans l'en-tête, jamais dans une table de travail ni dans
  le journal ; un refus = `import-key-required`), 500 règles `derive` au plus. Supprimés :
  `conflict_feed` du mapping, `fetch_conflicts`, le jeton ACLED en Rust, les tables et
  l'agrégateur de `conflict_events.rs` (891 → 50 lignes : reste la règle de confirmation d'une
  piste, appliquée après tout import qui écrit des `geo_event`).

| Écart | Où il se lève |
|---|---|
| Date de premier signalement UCDP : le minimum des 8 premières dates de `source_date` (`least` sur `split_part`, le worker n'a pas les tableaux) | fonctions de tableaux dans la liste blanche du worker, si un cas le demande |
| Un en-tête CSV absent est un changement de forme, même pour une colonne `optional` (qui tolère une cellule vide) : un export UCDP sans `adm_1` échouerait | à garder : c'est le signal voulu |
- **2026-11-13** : **étape `call` (Starlark)**, choix de Shan pour la presse GDELT. Le bac à sable
  des fonctions passe dans `pnex-core` (feature `starlark`, réexporté tel quel par le nœud de
  flow : même code, mêmes limites) ; `call_rows` : `handle(rows)` reçoit la table de travail en
  liste de dicts (valeurs texte) et rend les lignes de la suivante. Elle tourne **dans le worker**
  comme une requête (file `pipeline_step_jobs`, colonnes `kind` et `settings`, migration
  000150), 5 min et 200 000 lignes au plus ; les tests de rejeu jouent la même fonction en
  processus. Le code vient d'une section `pipeline_functions:` du pack (jamais installée dans le
  registre des fonctions d'une org, dont le contrat `handle(inputs, msg)` diffère), nommée
  `<nom>` ou `<pack>/<nom>`. **Presse GDELT ×2** (`prospecting_b2b_fr/press_gdelt`,
  `markets/market_press`) : sociétés suivies par trois axes d'objets (`columns` : autres
  propriétés en colonnes), noms et mot le plus long par `companies/press_names`, recherche
  rapide en SQL (`strpos` sur les organisations), correspondance exacte et digest par
  `companies/press_digest` (titre central, lisible d'abord). Ajouts génériques : `quoted: false`
  (lignes brutes à tabulations, aussi pour GDELT événements), `enumerate`/`fetch` `for_each` sur
  une table issue d'un `sql` ou d'un `call`. Supprimés : `press_feed`, `fetch_press`,
  `pnex-core/src/gkg.rs` (472 lignes). Plus aucun flux codé en Rust dans les imports.

| Écart | Où il se lève |
|---|---|
| L'heure en cours n'est pas lue (fin des périodes exclue) : une tranche publiée dans l'heure attend le run suivant | à garder pour un run quotidien |
| Les lignes passent au bac à sable en JSON dans le texte du script : une table de 200 000 lignes larges peut dépasser le tas de 64 Mo | passage des lignes par valeurs Starlark si un cas le demande |
- **2026-11-13** : **ADS-B** en pipeline (`trade_flows/adsb_lol`, table `adsb_lol_positions`).
  Nouveau dans `fetch` : **`parts`** (le fichier = ces parties jointes dans l'ordre, chacune
  nommée par `{{part}}` dans l'adresse ; une partie absente après la première clôt le fichier) et
  **`stream: true`** (lu à mesure, jamais gardé : seulement dans un document `large`, suivi
  d'exactement un `unpack` tar puis d'un `read` à colonnes déclarées, seuls lecteurs de la table
  d'avant ; les entrées sont lues par lots de 64 Mo, le moteur exécute les trois étapes d'un
  tenant, plafond 64 Go). Une redirection d'un fichier en flux suit vers un hôte déclaré du
  document, en HTTPS seulement (les fichiers d'une release partent de l'hôte de stockage de la
  forge). `read` JSON lit un fichier gzip (une trace par avion, compressée ou non), `base` accepte
  `^/pointeur` (un instant du document, dans l'unité de `format`) et un compte fractionnaire
  (`72946.01` s → µs). **Supprimés** : `services/bulk_history.rs`, `pnex-core/src/bulk.rs`, la
  planification des partitions (`Period`, `plan_days`, `plan_years`, `settled`, `record`) ; la
  table `market_history_partitions` n'a plus d'écrivain (suppression PE9).

| Écart | Où il se lève |
|---|---|
| Les lignes typées d'un jour d'ADS-B passent par des fichiers Arrow temporaires avant l'écriture (plusieurs Go de disque du serveur le temps du run), l'ancien chargeur écrivait au fil de l'eau | `write` en flux si le disque manque |
| `icao` n'est plus mis en minuscules ni coupé (les traces de readsb le sont déjà) | à garder |
| Le verrou « un gros chargement par org » partagé avec l'historique des prix n'existe plus ; le verrou de run des pipelines (`PIPELINE_RUN`) le remplace | à garder |
| Une trace de plus de 64 Mo fait échouer le jour (l'ancien chargeur la sautait) | à garder : aucune trace réelle n'approche |
- **2026-11-13 (suite) — PE6 commencé : calendriers.** Cible **`write: time_ranges`** (plages de
  l'org, par `external_id`, colonnes `label`, `planned_start`, `planned_end`, `category`,
  `source_url`, `description`) ; lecteurs **`ical`** (un enregistrement par `VEVENT`) et **`lines`**
  (une ligne par ligne de texte). BLS, FOMC et BCE sont trois pipelines du pack `markets`
  (`bls_calendar`, `fomc_calendar`, `ecb_calendar`), chacun lu seulement si `markets.calendars` le
  cite (axe `derive`). Les pages de la Fed et de la BCE, lues par la position de leurs cellules,
  passent par deux fonctions Starlark du pack (`fomc_page`, `ecb_page`) ; l'heure de New York est
  convertie dans le `sql` (`local_to_utc`). L'historique FOMC 2009-2020 est une liste de valeurs du
  pipeline. **Supprimés** : `services/markets/calendar.rs`, `pnex-core/src/markets/calendar.rs`,
  `fomc_2009_2020.tsv`, leur ligne de `HOST_DEBT`.

| Écart | Où il se lève |
|---|---|
| Les instants des plages sont écrits en UTC (l'ancien code gardait le décalage de New York) ; même instant | à garder |
| Une page de calendrier qui change de forme fait échouer le run (l'ancien code passait en silence) | à garder : l'échec se voit dans l'historique |
- **Identité par LEI** : cible **`write: proposals`** (jamais d'écriture directe ; l'objet est
  retrouvé par la valeur d'une de ses propriétés, `key_property` + `key`, jamais par un id ; un lien
  par `link_type`, `to_type`, `to_property`, `to_key` ; `origin` et `confidence` sur la cible).
  `markets/gleif_identity` : chaque société à CIK sans LEI demandée une fois (registre D937, qui
  remplace la propriété `lei_state`, supprimée du pack), rapprochement des noms légaux par la
  fonction Starlark `gleif_match`. **Supprimés** : `services/markets/identity.rs`,
  `pnex-core/src/markets/same_company.rs`, leur ligne de `HOST_DEBT`.
- **Documents des dépôts et filiales déclarées** : cible **`write: media`** (texte en document de
  l'org, retrouvé par `source_url`, indexé, lié à un objet retrouvé par une propriété ; seulement
  quand le run a l'application : ordonnanceur et `store::run_in`, pas un import) et mode
  **`extract: { text: true }`** (texte lisible d'une page, parties cachées ôtées).
  `markets/sec_documents` (chaque dépôt lu une fois) et `markets/sec_subsidiaries` (Exhibit 21 par
  deux fonctions Starlark du pack, `ex21_link` et `ex21_subsidiaries`, tables de lieux comprises),
  suivi de l'import en mode `snapshot` « Declared subsidiaries (SEC Exhibit 21) ». **Supprimés** :
  `services/markets/documents.rs` (son balayage MT5 + prédiction passe dans `markets::spawn_scheduler`
  en attendant PE6/PE8), `pnex-core/src/markets/exhibit21.rs`, les propriétés `document_state` et
  `subsidiaries_state`, les constantes d'hôtes SEC/ESEF et leur ligne de `HOST_DEBT`.

| Écart | Où il se lève |
|---|---|
| Chaque run des filiales relit toutes les annexes 10-K de l'org (l'ancien code lisait chaque rapport une fois) : un run par semaine | un état par rapport si le volume le demande |
| Le lien d'une filiale retirée se ferme à la date du run, plus à la date du rapport qui l'a retirée | `valid_to` d'un lien tiré d'une colonne, si la date compte |
| Un import (sans application) ne peut pas écrire de documents (`step_unsupported`) | passer le contexte à l'import si un pack le demande |
- **PE7 commencé : requêtes nommées (D948).** Un pack déclare `queries:` (clé, paramètres typés
  `text`, `integer`, `number`, `date`, `timestamp`, `uuid`, `choice`, facultatifs = `NULL`, SQL
  `SELECT … {{param}}`). `GET /analysis/named` liste celles des packs installés ;
  `POST /analysis/named/{pack}/{key}` vérifie chaque valeur, l'écrit en littéral de son type (un texte
  entre apostrophes doublées : une valeur n'est jamais du SQL) et la passe à `analysis::submit`
  (droits, marquages et plafonds de D825-D826). `analysis:run`, outils MCP exposés, code
  `named-query-not-found`. Première requête : `markets/market_moves`. Les routes `/markets/*` ne sont
  pas encore remplacées.


## Ajouts du 2026-11-15 (marché du travail, `labor-market.md` §11)

Lecture `xlsx` (feuille par nom ou position), grant `client_credentials`,
corps JSON d'un `post` (`json_from`), pagination par plage
d'enregistrements (`pages.size`), 416 lu comme une réponse vide. Génériques :
aucun hôte ni format de source en Rust.

## 2026-11-17 — PE6 : marchés de prédiction en pipelines

Demande de Shan : finir PE6 par l'ingestion ; PneX collecte et sert, le modèle vit chez
market-advisor (les lectures qu'il consomme restent, rebranchées).

- **Moteur, générique** : `fetch.proxy` = nom d'une clé du coffre qui porte l'URL d'un proxy
  http(s) (source bloquée dans le pays de l'org) ; sans clé, appel direct ; clé posée mais
  illisible ou adresse refusée par la garde egress (R8) = échec, jamais d'appel direct de repli ;
  l'adresse du proxy n'apparaît dans aucune erreur. Interdit avec `stream`. `write.table.key` =
  colonnes qui nomment une ligne : une ligne dont la clé est déjà dans la table, ou répétée dans le
  run, est écartée (anti-jointure jouée par le worker sur la table de l'org ; ajout seulement).
  Une clé de coffre posée mais illisible est désormais gardée vide pour toutes les étapes : le
  `fetch` refuse (`secret_required`) au lieu d'appeler sans elle.
- **Pack `prediction_markets` 2.0.0** : quatre pipelines — `markets` (import quotidien
  « Prediction markets » : objets marché et un objet par état de résolution atteint, jamais
  modifié ensuite, lien `resolution_of`), `prices` (probabilités horaires par jour clos depuis
  `history_from`), `prices_fine` (minute par minute, 7 derniers jours, slugs de `fine_markets`),
  `trades` (1 500 plus récents par marché toutes les 20 min, clé `tx, wallet, token, side, ts`).
  Fonction Starlark `polymarket_markets` (listes en JSON texte dans le JSON, état dérivé).
- **Supprimés** : la passe Rust (client, proxy, curseurs, écritures), les décodeurs et les hôtes de
  `pnex_core::markets::prediction`, leur ligne de `HOST_DEBT`, les tables PostgreSQL
  `prediction_prices`, `prediction_trades`, `prediction_cursors` (migration 000154, rechargement de
  zéro, D952). Les entrées du modèle, les scores face au marché et le détecteur de mises précoces
  lisent les tables de l'org.

| Écart | Où il se lève |
|---|---|
| Objets marché et résolutions mis à jour une fois par jour (récurrence d'import), l'ancienne passe relisait chaque heure | un ordonnanceur d'imports à l'heure, si un cas le demande |
| Probabilités horaires lues une fois le jour clos (retard ≤ 24 h), l'ancienne passe lisait jusqu'à l'heure en cours | un `write` à clé sur le jour en cours, si le retard gêne |
| Un état repassé (litige → proposé → litige) n'est gardé qu'une fois par couple état/issue | à garder |
| Échanges : 3 pages de 500 par marché et par run ; un marché plus actif perd les échanges entre deux runs | `pages` par décalage, après vérification des paramètres réels de l'API |
| `fine_markets` ne reconnaît que les slugs (filtre `codes` sur une seule propriété) | à garder |
| Les anciens objets `prediction_resolution` (propriété `market` en référence) restent dans les orgs qui les avaient | à supprimer à la main ; les nouveaux portent `condition_id` |
| Formes des réponses et paramètres de l'API non vérifiés en réel (hôtes et documentation bloqués depuis la France) | premier passage réel par le proxy ou un VPN |

### 2026-11-17 (suite) — PE6 : fin des appels en dur après import (D946, D949)

- **Confirmation des pistes (D741)** : pipeline `world_events/lead_confirmations` (objets
  `geo_event` lus par l'axe d'objets, rapprochement pays / jour ± 1 / nature compatible en SQL)
  et import « World event confirmations » qui ne fait que mettre à jour les pistes
  (`create: false`, `update: true`) ; quotidien, avec `after: [gdelt_events]` : l'import GDELT
  n'a plus de récurrence propre et tourne juste avant (ordonnanceur à dépendances D575). UCDP et
  ACLED gardent leur rythme hebdomadaire, leurs événements sont pris au passage suivant.
  **Supprimés** : `services/markets/world.rs`, `pnex_core::conflict_events`, l'appel dans
  `imports::write_locked`. *Écart D998 (2026-11-18)* : ce rapprochement est de
  l'organisation, à refaire en flow (D998-T5).
- **Séances anormales (D735–D738)** : pipeline `markets/market_moves` en SQL seul (variations
  entre jours cotés, écart type et moyenne des 250 variations précédentes par fenêtre glissante,
  60 au moins, seuil `max(k·σ, plancher)`), lisant les tables `fred_move_series`, `bis_series`,
  `alfred_move_series`, `tiingo_move_tickers` ; import « Abnormal sessions » (création seule de
  `market_event`, lien `event_about` vers la série ou l'instrument), quotidien, `after:
  [fred_move_series]` (l'import FRED surveillé n'a plus de récurrence propre). L'avis par séance
  devient une règle de l'org sur `market_event` (D949, choix de Shan) ; `move_notify_channels`
  ne sert plus qu'à la synthèse quotidienne (D814). **Supprimés** : détection, avis et appel en
  dur de `services/markets/moves.rs`.
- **Moteur, générique** : `sql.columns` (une table de l'org déclarée dans `tables` et absente se
  lit vide avec ces colonnes, en texte ; sans elles la requête ne donne aucune ligne) ; cas de test
  `tables:` (table de l'org → fixture CSV du pack, lue par le rejeu) ; `given` n'est requis que si
  le document lit le réseau ; `ln`, `exp`, `sqrt` dans la liste blanche du worker.

| Écart | Où il se lève |
|---|---|
| Une séance d'une série sans date de publication (FRED ou BIS sans ALFRED) est datée de sa lecture et marquée `backfilled` au-delà de 5 jours ; l'ancien code la marquait seulement à la toute première lecture | à garder |
| Un avis par événement (règle) au lieu d'un avis par séance listant ses mouvements | une règle groupée, si un cas le demande |
| Un événement archivé par un membre peut être recréé au passage suivant (l'import ne voit que les objets vivants) ; vrai pour tout import | clé d'unicité sur les archivés, générique |
| Si l'import FRED surveillé échoue, la recherche du jour attend (une étape en échec bloque ce qui en dépend) ; idem GDELT pour les confirmations | à garder : l'échec se voit dans les builds |
| Tiingo, ALFRED et BIS sont lus par leurs propres imports : leurs séances sont trouvées au plus un jour plus tard | `after` sur eux quand leurs clés sont posées |
| Les pipelines des packs restent hors de l'ordonnanceur à dépendances (balayage par minute) ; `after` d'un document de pipeline n'est pas appliqué (aucun pipeline livré ne s'en sert) | nouveau type de cible de build, avec le contrat de `/data-builds` |
| Il reste deux balayages de domaine : la boîte des terminaux MT5 (routes `/mt5` gardées, choix de Shan) et la synthèse de séance (D814) | PE8 |


## 2026-11-17 — PE5 : conception de `browse` (D995–D996, choix de Shan)

### D995 — Une étape `browse` met le run en pause, il reprend sans worker bloqué

Le moteur lance la navigation (navigateur serveur, ou agent de l'extension si `session: required`,
D782) et le run passe à l'état `waiting` ; la fin de la navigation reprend le run à l'étape
suivante, avec la table `as` = une ligne par ligne d'entrée, une colonne par extrait. Pour que la
reprise ne sauve qu'une ligne et un numéro d'étape, `browse` suit directement l'énumération
(`for_each` = la table de l'`enumerate`, aucune autre étape avant) ; refus `browse_position`
sinon. Un redémarrage du serveur ne perd rien : la navigation finie trouve son run en base. Une
navigation sans réponse au-delà de son plafond (celui des runs d'agent) échoue la ligne, jamais
le run. `keep: false` efface les extraits de la navigation dès la reprise (relations D976). Rejeu
des tests : `given` porte `browse:<as>` → fichier d'extraits par ligne ; aucun navigateur.

### D996 — Les finalités deviennent des étapes génériques

Capture de profil (D333), liste d'employés (D779), recherche d'une personne (D788) et relations
d'un contact (D976) quittent `site_recipes/purpose.rs` : chacune est un pipeline du pack
`prospecting_b2b` (navigation, puis `sql` / `call` / `write`), lancé par sa route avec les
identifiants des objets visés (`enumerate` sur l'axe d'objets restreint à ces ids). Les documents
captés passent par `write: media`, le score de rapprochement par une fonction Starlark du pack,
les rattachements par `proposals`, le filtrage par étiquette par `sql`. Les gardes des routes
(opposition, plafond par jour, état par utilisateur D977, effacement D341) restent sur la route :
elles décident si le run part, pas ce qu'il écrit.

> **Amendée par D998** (2026-11-18) : le pipeline **collecte** (navigation, documents captés,
> tous les candidats stockés) ; le score, le rattachement, les propositions et la relance
> d'une recherche ou d'une page sont de l'**organisation**, faite par un flow du pack
> déclenché par les données (retour au partage de D788). Le passage « le score de
> rapprochement par une fonction Starlark du pack, les rattachements par `proposals` » est
> levé. Le filtrage par étiquette des relations (D976) reste à la collecte : écart justifié
> par la minimisation (une personne hors sélection n'est jamais écrite).

### Découpage

| Lot | Contenu |
|---|---|
| PE5.1 | Moteur : `browse` (validation de position, pause, reprise, `keep`), rejeu par `given` ; envoi vers les moteurs de navigation existants |
| PE5.2 | Capture de profil et liste d'employés en pipelines (`write: media`) ; `Purpose::ProfileCapture`/`PeopleCapture` supprimés |
| PE5.3 | Recherche d'une personne — **refait le 2026-11-18 selon D998/D999** : le pipeline collecte (minimisation), trois flows du pack scorent, rattachent ou proposent, relancent ; service et routes Rust retirés |
| PE5.4 | Relations d'un contact en pipeline (filtre par étiquette en `sql` = écart justifié par la minimisation, D998 ; `keep: false` ; pages par `ask`) |
| PE5.5 | Règles des orgs migrées en pipelines ; `/site-recipes`, page « Sites web » et `default_rules.rs` supprimés (`feat(api)!`) ; dette d'hôtes de `browser_agent.rs` levée |

### 2026-11-17 (suite) — PE5.1 livré, PE5.2 : capture de profil

- **Moteur** : `browse` (état `waiting` de `pipeline_items`, migration 000155 ; reprise par la
  finalité `Pipeline`), `write: objects` hors import (planification D301, provenance
  `pipeline:<clé>`, pas de `snapshot`, une entrée d'un type absent de l'org est laissée avec ses
  liens), clé `@id` d'un objet existant (jamais créé), axe `asked: [colonnes]` et
  `store::run_asked` (une ligne demandée est reprise même faite, sauf en attente). `write: media`
  gagne `stamp` (l'id du document posé sur l'objet), `keep_days` (purge par `purge_after`) et la
  recherche par `id`.
- **Capture de profil** : six pipelines `prospecting_b2b/profile_capture_<réseau>` (pack 4.2.0,
  lien `captured_text` personne/organisation → document). La route lance le pipeline du réseau
  dans l'agent du membre ; la liste des captures lit les navigations des pipelines.
  **Supprimés** : `Purpose::ProfileCapture`, `profiles::profile_captured`,
  `default_rules::profile_capture`.
- **Liste d'employés** (D779) : pipeline `prospecting_b2b/people_capture_linkedin` (document de
  l'organisation, tampon `people_to_read`). **Supprimés** : `Purpose::PeopleCapture`,
  `profiles::people_captured`, `default_rules::people_capture`, `PEOPLE_SCROLLS`.

| Écart | Où il se lève |
|---|---|
| Le document capté est lié par `captured_text` (objet → document), plus par `published_by` (document → personne) | à garder |
| L'étape `sql` d'une capture passe par le worker de pipelines : sans worker, la ligne attend | à garder (le worker tourne partout) |
| Un import ne peut pas naviguer (pas de contexte d'application) | passer le contexte aux imports si un pack le demande |

### 2026-11-18 — PE5.3 : recherche d'une personne en pipeline (D997, choix de Shan)

### D997 — Une ligne peut redemander le pipeline (`write: ask`)

Une étape `write: { from: <table>, ask: {} }` redemande au même pipeline les lignes de la table
(colonnes de son axe `asked`), comme une route les demande. Elles partent une fois le run fini et
le verrou des pipelines de l'org rendu, dans l'agent du même membre (la finalité `Pipeline`
porte `user`). Refus `ask_without_asked` si le document n'énumère pas un axe `asked` suivi.
C'est l'échelle des requêtes d'une recherche de personne (une requête par ligne, la suivante
demandée tant que rien ne score) ; les pages des relations d'un contact (PE5.4) s'en serviront.

> **Selon D998** (2026-11-18) : le mécanisme reste (une ligne demandée au pipeline), mais
> qui décide de demander la suite est la couche d'organisation (un flow, D998-T2), pas le
> score calculé dans le pipeline de collecte. La pagination pure (page suivante tant
> qu'il y en a) reste un usage de collecte admis.

- **Moteur** : `write: ask` ; une ligne de `write: proposals` peut porter sa `confidence` ; une
  table de navigation vide garde ses colonnes `name`, `text` ; les cartes d'un extrait arrivent au
  pipeline en tableau JSON dans `text` ; une carte qui pointe vers un profil opposé (D341) est
  retirée dès la réception, comme un extrait.
- **Recherche d'une personne** : pipeline `prospecting_b2b/person_match_linkedin` (pack 4.3.0) :
  navigation (barre de recherche, cinq cartes, trois profils ouverts), score par la fonction
  Starlark `person_match_score` (mêmes poids qu'avant : nom 30, société actuelle 50, passée 25,
  groupe 15, ville 10), meilleur profil au-dessus du seuil rattaché (`uses_identifier`, texte en
  `captured_text` tamponné `profile_to_read`), plausibles en propositions
  (`prospecting_b2b.person_match`), sinon requête suivante par `ask`. La route garde l'opposition,
  le plafond par jour et construit le sujet (mots repliés, échelle, seuils) ; le passage horaire
  lance les cibles et prospects et suit les décisions (profil accepté → texte à lire, tout
  refusé → requête suivante). La vue d'une personne se lit des navigations du pipeline, des
  propositions et des liens. **Supprimés** : table `person_matches` (migration 000156),
  `Purpose::PersonMatch`, `search_finished`, le score et les candidats de
  `pnex_core::person_match`, `default_rules::person_search`.

| Écart | Où il se lève |
|---|---|
| SQL des pipelines = DataFusion sans fonctions JSON : le regroupement des extraits et la requête suivante sont dans la fonction Starlark | ajouter des fonctions JSON au worker si d'autres pipelines en ont besoin |
| Le repli des accents de la fonction Starlark couvre le latin courant, pas toute la table de `fold.rs` | passer le texte replié par le moteur si un réseau non latin est visé |
| La vue d'une recherche ne liste plus les candidats écartés ni le score du profil rattaché | écrire les candidats dans une table du pipeline si l'UI en a besoin |
| Une recherche dont la reprise échoue (étape `sql`) reste « en recherche » dans la vue | lire l'état de la ligne du pipeline dans la vue |

### 2026-11-18 — PE5.3 refait selon D998 : la recherche d'une personne en flows (D999)

### D999 — Collecte en pipeline, tout le reste en flows (choix de Shan, 2026-11-18)

Choix de Shan (assistant de questions) : **tout** passe en flows, y compris le choix des
personnes à chercher, le sujet lu dans le graphe et le suivi des propositions décidées ; un
profil LinkedIn est seulement un identifiant lié à la personne (une personne peut en avoir
plusieurs), aucune carte de candidat n'est gardée.

**Deux nœuds de flow génériques** :

- `ontology_trigger` — source d'événements d'un type d'objet : objet créé ou modifié (avec,
  en option, les propriétés surveillées), lien ouvert ou fermé qui le touche (types de lien),
  proposition décidée (origine). Le backend publie ces événements sur `pnex:onto:v1:{org}`
  depuis le journal des changements (D184), au mieux, sans rejeu : un événement manqué
  pendant un arrêt est perdu, comme `dataset_trigger`. Le message nomme l'objet, jamais ses
  propriétés : le flow relit avec `ontology_query`.
- `pipeline_ask` — demande à un pipeline les lignes de `msg.payload` (colonnes de son axe
  `asked`, D997), comme une route. Le serveur relit la configuration déployée (jamais la
  requête), plafonne par jour (`per_day`, plafond compilé), refuse une ligne dont un objet
  est sous opposition (D341). Le navigateur est l'agent de **celui qui a déployé** le flow,
  ou un agent commun (D550) : un flow ne pilote jamais l'agent personnel d'un autre membre.

**Le pipeline** `prospecting_b2b/person_match_linkedin` ne fait que collecter : une requête,
les cartes, trois profils ouverts. Il ne garde que les profils dont la carte nomme la
personne (filtre de minimisation, pas de score) et écrit pour la personne un document de
lot (JSON : requête, profils retenus, textes) tamponné `match_to_score`, gardé 2 jours.
S'il n'en garde aucun, le flow demande la requête suivante.

**Les flows du pack** (`prospecting_b2b`) :

- *Prospecting · identify people* : action `find_profile` de la personne (bouton de sa
  fiche) ou passage horaire (personnes des comptes `target`/`prospect`, sans compte
  LinkedIn, non cherchées depuis N jours) → `ontology_query` (la personne, ses liens
  `employs`, ses sites, son groupe) → fonction Starlark *sujet* (mots repliés, échelle) →
  `pipeline_ask`.
- *Prospecting · score profiles* : `ontology_trigger` sur `match_to_score` →
  `document_text` → fonction *score* (mêmes poids) → rattacher (le profil retenu est capté
  par `profile_capture_linkedin`, qui pose `uses_identifier`), proposer (`link_upsert`
  `propose`), ou requête suivante ; état de la recherche sur la personne (`match_state`,
  `match_step`, `match_pending`, `match_searched_at`).
- *Prospecting · follow decisions* : `ontology_trigger` sur les propositions décidées
  (origine `prospecting_b2b.person_match`) → acceptée : capture du profil ; toutes
  refusées : requête suivante. Un changement de la personne (société, ville) relance la
  recherche depuis la première requête.

**Supprimés** : `services/person_match.rs` (sujet, garde, balayage, suivi), routes
`/extension/people/{id}/person-matches` (rupture `feat(api)!`), types
`pnex_core::person_match`, panneau « Recherches » de la fiche ; l'état se lit dans les
propriétés `match_*` de la personne et les passages du pipeline.

| Lot | Contenu |
|---|---|
| D999.1 | `ontology_trigger` : publication backend, nœud runtime, configuration, éditeur, `NodeDoc`, i18n, tests |
| D999.2 | `pipeline_ask` : route interne, plafond, opposition, agent du déployeur, nœud, éditeur, `NodeDoc`, i18n, tests |
| D999.3 | Pipeline réduit à la collecte ; fonctions Starlark sujet, score, suivi ; trois flows du pack ; action `find_profile` ; propriétés `match_*` |
| D999.4 | Retrait du service Rust, des routes et du panneau ; contrat (OpenAPI, MCP, IAM, CLI), fiches de l'assistant |

**Livré le 2026-11-18** (D999.1 à D999.4, non commité à l'écriture). Écarts et compléments :

- Les extensions de type d'un pack portent aussi des **actions** (`TypeExtension.actions`) : c'est
  ainsi que `prospecting_b2b` ajoute `find_profile` à la personne. *Raccourci* : une action qu'une
  version suivante du pack retire reste sur le type (les actions n'ont pas de propriétaire).
- Le tampon d'un `write: media` (`stamp`) est journalisé (`object.stamp`) : sans cela aucun flow
  n'entendait la fin d'une collecte.
- Les réglages `match_write_threshold`, `match_propose_threshold`, `match_retry_days` sont lus par
  les fonctions du pack (`pack_setting`) ; `match_searches_per_day` disparaît : chaque nœud
  `pipeline_ask` porte son plafond (20 par jour par défaut, 3 nœuds pour la recherche).
- Les propositions d'un flow ont pour origine `flow:<id>@<version>` : le suivi filtre donc les
  propositions par type de lien (`uses_identifier`), pas par origine.
- Tests : `tests/extension.rs` (demande, refus, navigation, tampon, événement), rejeu du pipeline
  (`kept_urls`), `pnex-core/tests/pack_person_search.rs` (sujet, score, suivi, dates dans le bac à sable).
- Non vérifié : les trois flows déployés sur un vrai runtime (seuls leurs morceaux sont testés), et
  la recherche sur une vraie session LinkedIn.

### 2026-11-18 (soir) — Essais réels en local : recherche, relations du membre, affectation des entreprises (D999, suite)

Essais sur la pile locale avec l'agent navigateur de Shan (session LinkedIn réelle). Ce qui a été
trouvé et corrigé :

- **La page de résultats de LinkedIn a changé** (sondage du 2026-11-18) : plus de listes (`li`),
  plus de bouton de filtre « Personnes ». La recherche ouvre directement
  `/search/results/people/?keywords=…` et lit deux extraits : les liens de profil
  (`main a[href*='/in/']`) et le texte de la page. Une personne y est un bloc
  « nom • degré », titre, lieu ; le lien dont le texte est le nom donne son adresse. Aucun profil
  n'est plus ouvert : le titre de la carte suffit au score (nom 30, société 50, ville 10).
- **Le profil rattaché remplit la personne** : poste, société, ville lus du titre (« … chez X »,
  « … at X », « … | X » si X ressemble à un nom d'entreprise, « … de X » d'un seul mot).
- **Les relations du membre** (demande de Shan) : pipeline `prospecting_b2b/my_connections_linkedin`,
  une page de la recherche des relations de premier niveau par ligne demandée (10 personnes par
  page, 100 pages au plus chez LinkedIn, soit 1 000 personnes), chaque personne écrite par son
  adresse de profil (`profile_url`, clé unique) avec poste, société et lieu. Flow
  *Prospecting · my LinkedIn connections* : une fois au déploiement, puis chaque matin, 50 pages
  par jour (plafond des navigations d'agent : 100 par jour et par org).
- **Affectation des entreprises** (couche 3) : flow *Prospecting · employers from profiles*,
  déclenché par la propriété `profile_company` d'une personne : l'entreprise est retrouvée par sa
  clé de nom repliée (`name_key`, nouvelle propriété unique de l'organisation dans
  `prospecting_b2b`) ou créée, **même absente du registre SIREN** (choix de Shan), puis la
  personne y est rattachée par `employs` avec son poste. Une entreprise déjà employeuse (même nom
  replié) n'est pas dupliquée.
- **Moteur** : des lignes demandées explicitement (route, `pipeline_ask`) sont toutes prises ;
  le plafond par passage par défaut (10) les tronquait sans le dire. Un document peut toujours
  déclarer son `per_run`.
- **Pack** : la proposition d'un profil passe par le pipeline `person_match_propose` (un
  `object_upsert` ne peut pas viser `identifier.value`, qui n'est pas unique) ; une personne dont
  l'adresse LinkedIn est connue (`profile_url`) n'est jamais recherchée automatiquement (une
  cascade de recherches inutiles a été arrêtée par le plafond du nœud) ; un profil déjà rattaché
  retrouvé laisse la personne « Trouvé ».
- **Front** : le formulaire « Ajouter un lien » paniquait sur un type de lien listé deux fois.

| Écart | Où il se lève |
|---|---|
| Le code d'une fonction de flow est figé au déploiement : une mise à jour du pack demande de redéployer ses flows (le bandeau « à redéployer » le dit) | à garder, ou redéploiement proposé à la mise à jour du pack |
| L'injection unique (`once_delay_secs`) se redéclenche à chaque démarrage du moteur : la collecte des relations repart à chaque redémarrage du serveur | remplacer par une action « Lire mes relations » ou un déclencheur manuel |
| La recherche des relations plafonne à 1 000 personnes (100 pages de LinkedIn) | lire la page « Relations » en défilement, si la limite de 50 cartes par extrait est relevée |
| Lecture du titre : heuristique (des titres sans « chez/at/| » ne donnent pas d'entreprise ; quelques faux noms passent) | lexique à enrichir sur les cas réels ; un humain corrige le lien |
| L'état « je suis en relation » (D977) n'est pas posé par la collecte (table Rust, pas un objet) | écrire l'état depuis le pipeline, ou faire de la relation un lien `knows` membre → personne |
