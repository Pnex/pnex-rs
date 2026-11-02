# Recherche et décision d'investissement — PRD (D523–D536)

| | |
|---|---|
| Statut | Proposé le 2026-10-31 ; **codé le 2026-11-02 : MK0 partiel, MK1, MK2 (presse = piste), MK4, MK5, MK6 sur dépôts, D536 outils de lecture**. Reste : §7 bis |
| Numérotation | **D523…D536, D540…D546**, lots **MK0…MK6** |
| Repose sur | ontologie (D176–D191, séries D185, provenance D184), `companies` / `companies_fr` (D304–D306), pipelines (`data-pipelines.md` D344–D358), règles de sites (`site-recipes.md` D323–D328), ingestion média (`media-ingest.md` D159–D175), dossiers (`cases.md`), moteur financier (`due-diligence.md` D478), propositions (`proposals.md` D371–D377), nœuds `anomaly` / `forecast`, notifications (D49–D68), assistant (`ai-assistant.md` §9) |
| Sécurité | `security.md` §6 avant chaque lot ; sources sortantes via `pnex_core::egress` (R8), clés d'API au coffre (D113) |

> Avertissement produit à afficher sur chaque écran du domaine : les indicateurs, événements et tests sont des aides à l'analyse, pas un conseil en investissement ni une recommandation.

## 1. Le besoin

Outiller la démarche d'une équipe d'analyse sectorielle :

1. **Couvrir** une liste de sociétés (30 à 300) et leurs instruments cotés.
2. **Voir passer** tout ce qui les concerne : articles, communiqués, publications réglementaires, appels de résultats, données macro.
3. **Transformer** ce flux en **événements datés et sourcés** (résultats, révision de prévisions, opération sur capital, changement de dirigeant, litige, notation).
4. **Mesurer** la réaction du cours autour de chaque événement.
5. **Écrire une thèse** avant de décider (catalyseurs datés, critères d'invalidation, taille), tenir un **journal**, faire la **revue** après coup.
6. **Tester** une règle (« après une révision à la hausse, sur-performance à 20 jours ? ») sur des données **telles qu'elles étaient connues à la date** (point-in-time), sans biais d'anticipation.

Hors du besoin : prix temps réel des actions, exécution d'ordres, messagerie entre acteurs, diffusion de signaux.

## 2. Ce qui existe déjà

| Brique | Où | Rôle ici |
|---|---|---|
| Objets, liens, provenance, historique | ontologie 0.2.0 | émetteurs, instruments, événements ; `ontology_objects_changes` donne l'état à une date |
| Propriétés `series` (D185) + `telemetry/aggregate` | O2 | cours et volumes d'un instrument, lus par objet |
| Sociétés, SIREN, dirigeants, BODACC | packs `companies`, `companies_fr` | l'émetteur **est** l'organisation déjà connue |
| Imports déclaratifs et pipelines | D301, D344–D358 | référentiels (LEI, ISIN), cours de clôture, filings |
| Collecte web gardée | `site-recipes.md`, crawl | pages de communiqués, sites d'information sans flux |
| Audio / vidéo → texte | `media-ingest.md`, ASR | podcasts, webcasts de résultats |
| Recherche documentaire | `doc-search.md` | recherche plein texte des articles collectés |
| Extraction LLM à citation vérifiée | `due-diligence.md` D476 | même règle pour les événements (citation retrouvée ou rejet) |
| File « À valider » | `proposals.md` | les événements extraits sont des propositions |
| Ratios financiers | `pnex_core::finance` (D478) | fondamentaux des émetteurs (lot DD2) |
| Détection d'anomalies, prévision | nœuds `anomaly`, `forecast` | volume ou cours anormal → alerte |
| Dossier, chronologie, preuves | `cases` | support de la thèse |

## 3. Ce qui manque

- un référentiel **instrument / cotation / émetteur** avec identifiants (ISIN, LEI, FIGI, MIC) ;
- l'import des **cours de clôture** avec **opérations sur titres** (divisions, dividendes) ;
- le type **article** relié aux émetteurs, et le type **événement de marché** ;
- un calcul **d'étude d'événement** (rendement anormal) ;
- un dossier **thèse d'investissement** avec journal et positions fictives ;
- une lecture **point-in-time** pour les tests.

## 4. Principe d'architecture

```mermaid
flowchart LR
  subgraph Sources
    R[flux RSS / Atom]
    F[filings réglementaires]
    W[pages web<br/>site-recipes]
    A[audio / webcasts<br/>media-ingest]
    P[cours de clôture<br/>pipelines]
    G[référentiels LEI / ISIN / FIGI]
  end
  G --> I[instrument ↔ émetteur]
  P --> S[séries O2<br/>par instrument]
  R & F & W & A --> D[article<br/>média + texte]
  D --> L[liaison aux émetteurs]
  L --> X[extraction d'événements<br/>citation vérifiée]
  X --> Q[À valider]
  Q --> E[événement de marché]
  E & S --> ES[étude d'événement<br/>pnex_core::markets]
  E & S --> T[dossier thèse<br/>journal + positions fictives]
  E & S --> B[test point-in-time]
  S --> N[anomaly → notification]
```

## 5. Décisions

### D523 — Un pack `markets`, l'émetteur reste l'organisation

Pack `markets` (`requires: [companies]`). Types : `instrument` (action, obligation, ETF, indice, crypto-actif, devise ; ISIN, FIGI, devise de cotation), `listing` (instrument × place, MIC, ticker), lien `emet` (`organization` → `instrument`), `index_member` (instrument → indice, avec dates). Pas de type « société cotée » : une organisation devient émettrice par son lien. Couche pays éventuelle `markets_fr` (place Euronext Paris, code AMF) selon la règle des packs.

### D524 — Référentiels par imports, jamais saisis à la main

LEI (GLEIF, données ouvertes) → `organization.lei` ; ISIN ↔ LEI (fichier de correspondance GLEIF) ; FIGI et tickers (OpenFIGI, clé d'API au coffre) ; référentiel d'instruments européen (ESMA FIRDS). Chacun est une import D301 / pipeline D344 avec `skip_if_unchanged`. Rapprochement émetteur ↔ organisation existante par LEI puis SIREN, sinon proposition.

### D525 — Cours de clôture dans O2, bruts + facteurs d'ajustement

Une série par instrument et par champ (`close`, `open`, `high`, `low`, `volume`), labels `object=instrument/<id>`. On stocke le **brut** ; les opérations sur titres sont des objets `corporate_action` (date, type, ratio) et l'ajustement se calcule à la lecture. Raison : un ajustement réécrit dans la série casse le point-in-time. Granularité V1 : quotidienne. Crypto : la même chaîne, granularité minute possible (sources publiques gratuites).

### D526 — Une source = une import ou une règle de site, liste blanche par org

Pas de collecteur dédié : les flux RSS/Atom, filings et API de cours passent par les pipelines ; les pages sans flux par `site-recipes`. Liste des sources par org, chacune avec sa licence notée (champ `license_note`) et son rythme. Egress R8, `robots.txt` respecté, pas de contournement de paywall.

### D527 — L'article est un média, son texte reste privé à l'org

Un article = un `media_asset` texte (titre, source, URL, `published_at`, `fetched_at`, langue) indexé par la recherche documentaire. Jamais exposé par lien public ni par page publiée (`pages.md`, studio S5) : la conservation du texte sert l'analyse interne seulement. Purge par rétention d'org.

### D528 — Liaison article → émetteurs déterministe d'abord

Par ordre : identifiants présents (ISIN, LEI, ticker avec place), nom exact ou alias de l'organisation, puis LLM usage `extraction` sur les cas ambigus → proposition. Un lien porte son mode (`id`, `alias`, `llm`) et son score.

### D529 — L'événement de marché est un objet, toujours sourcé

Type `market_event` : `kind` (enum fermée minuscule : `earnings`, `guidance`, `capital_operation`, `m_and_a`, `management_change`, `rating`, `litigation`, `regulatory`, `macro`, `other`), `direction` (`positive`, `negative`, `neutral`, `unknown`), `event_at`, `published_at`, `known_at`, liens `concerne` → émetteur/instrument, `source` → article. Extraction : schéma JSON du pack (`extractors:`, comme D476), chaque champ porte `quote` retrouvée verbatim dans l'article, sinon rejet. La direction est celle **dite par la source**, pas un score de sentiment opaque.

### D530 — Événements : propositions, acceptation automatique possible par règle

Les événements extraits passent par la file « À valider » (D371). Pour un volume élevé, une règle d'org D377 peut accepter automatiquement une classe (`kind` + source), tracée. Un doublon (même émetteur, même `kind`, même jour, articles différents) fusionne ses sources au lieu de créer un second événement (empreinte D372).

### D531 — Trois horodatages, le test lit `known_at`

`event_at` (quand c'est arrivé), `published_at` (quand la source l'a dit), `known_at` (quand PNEX l'a su : `fetched_at` de l'article ou date d'import). Toute lecture « à la date t » filtre `known_at <= t` pour les objets et `ts <= t` pour les cours bruts. Un événement importé après coup (rattrapage d'historique) garde `known_at` = date d'import et est marqué `backfilled` : il est exclu des tests par défaut, inclus seulement sur choix explicite affiché.

### D532 — Étude d'événement : calcul pur dans `pnex_core::markets`

Fonction pure (cours, indice de référence, date, fenêtres) → rendement, rendement anormal (modèle de marché simple : rendement − β × rendement de l'indice, β estimé sur [−250, −30] jours), cumul sur fenêtres [−1, +1], [0, +5], [0, +20], et volume relatif. Agrégat sur un ensemble d'événements : moyenne, médiane, part positive, intervalle par bootstrap, effectif affiché. Testé sur des séries synthétiques à résultat connu.

### D533 — Le dossier « thèse » étend `case`

Pack `investment_thesis` (`requires: [markets, investigation]`) : `case_kind = investment`, instrument(s), sens (`long`, `short`, `watch`), date d'ouverture, **catalyseurs datés** (objets reliés, échéance), **critères d'invalidation** écrits, horizon, taille prévue. Journal = événements de dossier existants (note, décision, révision). Revue de clôture obligatoire pour fermer le dossier : résultat, ce qui était juste, ce qui ne l'était pas. Le dossier se gèle à chaque décision (snapshot existant) : la thèse d'origine ne se réécrit pas.

### D534 — Positions fictives, aucun lien vers un courtier en V1

`paper_position` (instrument, sens, quantité, prix et date d'entrée, sortie) reliée au dossier ; P&L calculé sur les clôtures. Import CSV d'un relevé de courtier possible plus tard (lecture seule). Jamais d'ordre passé par PNEX : exclu de l'API exposée à l'assistant et au MCP sous le motif `physical`/`outbound`, au même titre que les commandes de devices (D123).

### D535 — Test de règle = requête point-in-time + étude d'événement

Une règle = filtre d'événements (`kind`, `direction`, univers d'instruments, période) + fenêtre. Le moteur rejoue jour par jour avec la lecture D531, applique D532, rend la distribution et la comparaison à un tirage aléatoire de dates sur le même univers. Affichage obligatoire : effectif, période, part d'événements `backfilled`, univers (biais du survivant : un instrument radié reste dans l'univers à ses dates). Pas d'optimisation de paramètres en V1 (sur-ajustement).

### D536 — Assistant : lit, résume, propose ; ne décide pas

Outils de lecture (événements d'un émetteur, étude d'événement, état d'une thèse), synthèse quotidienne privée par notification, propositions d'événements. Jamais d'ordre, jamais de modification d'une thèse gelée, toute affirmation reliée à un article. Réponses marquées « aide à l'analyse ». Fiches `assistant-kb` à chaque lot.

### D540 — Les sources primaires d'abord, la presse détecte

Un chiffre se lit à sa source : dépôts SEC, séries FRED, puis banques centrales et publications réglementées. Le socle de calcul ne contient que des sources primaires. Un article sert à repérer un événement que les chiffres ne disent pas encore (litige, enquête, négociation) ; son contenu n'alimente aucun calcul et reste non confirmé tant qu'aucune source primaire ne le recoupe. Cela précise D526 et D527 : les flux d'articles passent après.

Mise en œuvre dans l'import D301, sans collecteur dédié : `shape` lit une réponse qu'aucun chemin ne décrit (`sec_tickers`, `edgar_filings`, `xbrl_facts`, `dated_csv`) ; `contact_setting` déclare un contact dans le `User-Agent`, sans lequel la SEC répond 403 et FRED coupe la connexion ; `per_setting_code` et `rows_in_setting` limitent la lecture à la liste de l'org, un registre entier n'étant jamais voulu. Chaque requête reste derrière le garde R8.

### D541 — Valeurs datées : on ajoute, on ne remplace jamais

Table `market_observations` (objet, indicateur, unité, période, `known_at`, valeur, source, `backfilled`), en PostgreSQL et non en objets (≈ 1 700 valeurs par société) ni dans O2 (D525 reste pour les cours : la lecture « dernière valeur connue à t » par période est une requête SQL directe, et la rétention O2 ne doit pas toucher un historique financier).

- Une valeur inchangée n'ajoute rien ; une valeur différente ajoute une ligne. Lecture à la date t = pour chaque période, la dernière ligne avec `known_at <= t`.
- Dépôt : `known_at` = fin de la journée de dépôt (UTC), jamais avant que le marché ait pu le lire.
- Source sans date de publication (FRED) : `known_at` = instant de lecture, et les premières valeurs d'un indicateur sont marquées `backfilled`, exclues d'une lecture datée sauf demande (D531).
- Les valeurs restent celles de la source, sous le nom de son concept. Mesuré sur un vrai dossier : le chiffre d'affaires change de concept selon les années, et une « révision » peut être un arrondi ou une erreur de balisage. Aucun rapprochement de concepts tant qu'un calcul n'en a pas besoin.

Limite connue : seuls les 1000 derniers dépôts (`filings.recent`) sont lus. Les valeurs d'époque FRED antérieures à la première lecture ne sont pas importées (l'API ALFRED demande une clé).

### D542 — Le document d'un dépôt devient un média de l'org

Une tâche de fond lit le document principal de chaque dépôt (20 par passage de 5 minutes, 150 ms entre deux requêtes, sous le plafond de 10 requêtes par seconde de la SEC), en garde le texte lisible, en fait un média indexé par la recherche documentaire (D527) et le relie par `filing_document`. Seules les orgs ayant déclaré un contact sont lues. L'état du dépôt (`document_state`) évite de relire. Le média reste privé à l'org.

### D543 — Cours : Tiingo, clé d'org au coffre, valeurs brutes (tranche Q-MK1)

Essais du 2026-11-02 : Stooq impose un contrôle JavaScript, Yahoo n'a pas d'API autorisée : écartés (pas de contournement, D526). Tiingo a une API officielle et une offre gratuite à usage interne personnel (50 requêtes/heure, 1000/jour), une offre commerciale à 50 $/mois ; sa couverture européenne n'est pas confirmée. Le format `daily_prices` reste indépendant du fournisseur. **Aucune clé n'était disponible : le chemin est testé sur un mock, jamais sur le vrai Tiingo.**

Corrige D525 : les cours vont dans `market_observations` et non dans O2, pour une seule lecture datée et sans rétention O2. On stocke `close`, `volume`, `dividend`, `split` bruts ; l'ajustement se calcule à la lecture.

Sécurité (R9) : une import nomme une source de `KEYED_SOURCES`, jamais un secret ni un hôte. Hôte et nom de secret sont fixés dans le code, l'URL est vérifiée avant de lire le coffre, la clé part en en-tête (absente des URL et des journaux), et reqwest la retire sur redirection vers un autre hôte.

### D544 — Les événements de marché sans LLM : les rubriques des dépôts

Un 8-K déclare déjà sa nature (2.02 résultats, 5.02 dirigeants, 1.01 accord, 2.01 acquisition). L'étude et le test de règle lisent directement ces rubriques, datées par le dépôt lui-même : pas de `backfilled` possible (D531). Cela couvre MK6 et une grande part de MK3 sans LLM ; l'extraction à citation vérifiée (D529) reste à faire, le mécanisme `extractors:` de D476 n'existant pas encore dans le code.

### D545 — Test de règle : pas de paramètre, toujours face au hasard

`rule-test` : formulaire + rubrique, fenêtres fixes, distribution du rendement anormal, comparée à 20 dates tirées par instrument (tirage déterministe, résultat reproductible), effectif affiché. Limite affichée : seuls les instruments suivis aujourd'hui sont mesurés (biais du survivant, D535 non tenu en entier : pas d'univers historique).

### D546 — Thèse : extension de `case`, position fictive mesurée

Pack `investment_thesis` conforme à D533–D534. Le résultat d'une position se calcule sur les clôtures, jamais sur un prix saisi. Limite : la revue de clôture est un champ, non exigée à la fermeture ; le gel passe par les snapshots existants, à la main.

## 6. Sources V1 (conditions d'usage à vérifier source par source)

| Source | Contenu | Accès |
|---|---|---|
| GLEIF | LEI, correspondance ISIN ↔ LEI | ouvert |
| OpenFIGI | FIGI, tickers | clé gratuite |
| ESMA FIRDS | référentiel d'instruments UE | ouvert |
| SEC EDGAR | filings US (8-K, 10-Q, 10-K) | ouvert, `User-Agent` déclaré, quota |
| Publications réglementées UE / FR | communiqués d'émetteurs, déclarations | ouvert |
| Banques centrales, statistiques publiques | taux, inflation, agrégats | ouvert |
| Flux RSS des médias | titre, chapô, lien | selon conditions de chaque éditeur |
| Pages de communiqués des émetteurs | texte des communiqués | `site-recipes` |
| Cours de clôture actions | OHLCV quotidien | **à choisir** : fournisseur branché par une clé au coffre (D113) |
| Places crypto | OHLCV, carnet | API publiques |

## 7. Lots

| Lot | Contenu | Valeur sans LLM |
|---|---|---|
| **MK0** | pack `markets`, imports GLEIF / OpenFIGI / FIRDS, rapprochement émetteurs | oui |
| **MK1** | cours de clôture + `corporate_action`, séries par instrument, fiche instrument (graphique, émetteur) | oui |
| **MK2** | sources d'articles (RSS, filings), type article, liaison D528 déterministe, fil par émetteur | oui |
| **MK3** | extraction d'événements à citation vérifiée, propositions, règle d'acceptation, alertes | non |
| **MK4** | `pnex_core::markets` (étude d'événement), widget sur la fiche émetteur et le dossier | oui |
| **MK5** | dossier thèse, journal, positions fictives, revue de clôture | oui |
| **MK6** | test de règle point-in-time | oui |

Premier ensemble utile : MK0 → MK2 + MK4 (suivre ses valeurs, lire leur fil, voir la réaction aux publications réglementaires), avant toute extraction LLM.

## 7 bis. Reste à faire (2026-11-02)

- Essai réel de Tiingo (clé à poser au coffre) et couverture européenne des cours.
- MK0 : GLEIF/OpenFIGI/FIRDS, rapprochement avec une organisation existante (aujourd'hui une société SEC est créée par CIK, un doublon est possible avec une société déjà connue par LEI ou SIREN).
- MK3 : extraction LLM à citation vérifiée (D476 absent du code), règles d'acceptation.
- Répartition du balayage des documents entre orgs ; millésimes FRED antérieurs à la première lecture ; dépôts plus anciens que `filings.recent` ; alertes `anomaly` sur volume ; synthèse quotidienne de l'assistant ; widget de fiche émetteur.

## 8. Hors périmètre V1

Prix temps réel des actions, carnet d'ordres actions, exécution, connexion courtier en écriture, options et dérivés (grecques, surfaces), données alternatives payantes, optimisation de portefeuille, diffusion ou partage de signaux hors de l'org.

## 9. Questions ouvertes

- **Q-MK1** : tranché pour les États-Unis (Tiingo, D543) ; Europe ouverte.
- **Q-MK2** : univers de départ (indices, liste personnelle) et profondeur d'historique pour MK6.
- **Q-MK3** : crypto en MK1 ou plus tard ; granularité minute = volume O2 à mesurer.
- **Q-MK4** : appels de résultats : webcasts publics par `media-ingest`, ou seulement les communiqués écrits en V1.
