# Marché du travail territorial — ingestion et couches carto (appendice, 2026-11-14)

| | |
|---|---|
| Statut | **Premiers lots codés (2026-11-15)** : moteur LM0, BMO, tension, offres (§11) ; le reste proposé |
| Déclencheur | Cibler des sites industriels selon les métiers en tension, la rotation et la pyramide des âges du territoire ; afficher le tout sur la carte, sous les établissements |
| Repose sur | `data-pipelines.md` (D344–D358, §9 D569–D578 datasets), `partner-watch.md` (pack `companies_fr`), `account-mapping.md` (D329–D343, `map_pin.headcount_band`, critères de score D336), `geo-layers.md` (L1–L28, phase A livrée le 2026-11-14), `secrets.md` (D110), `security.md` (R1, R2, R8) |
| Numérotation | décisions **M1–M18 provisoires**, renumérotées `Dxxx` au merge (D970–D975 pris au 2026-11-14 par `crm.md` et `watch.md`, D976–D977 par les relations LinkedIn (`account-mapping.md`) : prochain libre D978, à revérifier au merge) ; lots **LM0…LM5** |
| Exécution | **pipelines uniquement** : aucun flow, aucun nœud. Chaque source = un fichier YAML de pipeline dans un pack, planifié par `recurrence` |

## 1. Le besoin en une phrase

Charger dans PNEX les données publiques du marché du travail (tensions par métier, besoins de recrutement, offres d'emploi, âge et catégories sociales des actifs, départs en fin de carrière), les rattacher aux **établissements** (`map_pin`, SIRET) et aux **territoires**, et les afficher en **couches** sur la carte : zones colorées au fond, sites scorés au-dessus.

## 2. Non-objectifs

- Rien de propre à un vendeur ni à un secteur : le pack ne fixe ni métiers cibles ni pondération. Métiers suivis et poids du score sont des **réglages d'org** (école D336).
- Pas de données personnelles : les champs contact des offres (nom, téléphone, e-mail du recruteur) ne sont **jamais lus** (§8).
- Pas de donnée par entreprise que le public n'a pas : âge, rotation et catégories sociales restent **agrégés au territoire et au métier**. Le secret statistique est la frontière, pas une limite à contourner.
- Pas d'historique des offres antérieur au premier run : l'API ne sert que les offres actives. L'historique se construit à partir du jour où l'archive tourne (M7).

## 3. Sources

URLs et noms de fichiers **à confirmer au lot** (les producteurs renomment leurs fichiers chaque année) ; les hôtes sont ceux à déclarer dans `hosts:`.

| # | Source | Producteur | Grain | Rythme | Format | Hôte |
|---|---|---|---|---|---|---|
| S1 | Indicateurs de tension par métier (+ composantes : intensité d'embauche, manque de main-d'œuvre, conditions de travail, inadéquation géographique, lien formation-emploi) | DARES / France Travail | FAP × département (et région) | annuel (édition 2025 parue le 22/10/2026) | xlsx | `dares.travail-emploi.gouv.fr` |
| S2 | Enquête Besoins en main-d'œuvre (BMO) : projets, projets difficiles, saisonniers | France Travail | FAP2021 × bassin d'emploi | annuel (avril) | csv / xlsx | `www.data.gouv.fr`, `static.data.gouv.fr` |
| S3 | API Offres d'emploi (offres **actives**) | France Travail | offre (ROME, commune, lat/lon, entreprise, contrat, dates, texte) | quotidien | json (OAuth2 client credentials) | `entreprise.francetravail.fr` (jeton), `api.francetravail.io` |
| S4 | Recensement : actifs au **lieu de travail** par âge, sexe, PCS 2020, secteur | INSEE | commune → zone d'emploi | annuel (millésime RP) | zip csv | `www.insee.fr` |
| S5 | « Les métiers en 2030 » : départs en fin de carrière, postes à pourvoir | DARES / France Stratégie | FAP × (France, région) | ponctuel | xlsx | `dares.travail-emploi.gouv.fr` |
| S6 | Mouvements de main-d'œuvre (taux d'entrée/sortie) | DARES | secteur × territoire | trimestriel | xlsx / csv | `dares.travail-emploi.gouv.fr` |
| S7 | Référentiels : table de passage ROME ↔ FAP2021 ; commune → zone d'emploi 2020 ; commune → département ; commune → bassin BMO | DARES, INSEE, France Travail | — | à chaque refonte | xlsx / csv | idem |
| S8 | Contours : départements, zones d'emploi | IGN (ADMIN EXPRESS) / INSEE | polygones | annuel | GeoJSON / GeoPackage | `data.geopf.fr` (à confirmer) |
| — | Établissements (SIRET, NAF, tranche d'effectif, position) | **existant** : `companies_fr` + `prospecting_b2b_fr` | site | — | — | — |

Plus tard (hors V1) : MOBPRO (navettes domicile-travail, inadéquation géographique fine), Filosofi carroyé (revenus des habitants), La Bonne Boîte (potentiel d'embauche par établissement, à vérifier qu'elle est toujours exposée).

## 4. Modèle

```text
                  datasets (D569, pas d'objets)                 ontologie
 S1 tension ───► lm_indicators(metric=tension_index…) ───┐   (forme longue §4 bis,
 S2 BMO ───────► lm_indicators(metric=hiring_*) ─────────┤    pivots ISCO / NUTS)
 S4 RP ────────► lm_workforce(ze, pcs, age_band, nb) ────┤
 S5 2030 ──────► lm_indicators(metric=retirements_*) ───┼─► lm_site_signals ──► map_pin.(lm_* properties)
 S7 réf. ──────► lm_*_crosswalk, lm_sector_occupations ─┤      (sql, score)         + territory (zones)
 S3 offres ────► lm_job_postings (archive datée) ────────┘                          + job_posting ──posted_at──► map_pin
 S8 contours ──► geo_features (layers « zones »)  ◄── geo-layers phase A (livrée)
```

- **Statistiques = datasets**, jamais des objets (D570 : une ligne n'est pas forcément un objet). Quelques centaines de milliers de lignes de croisements FAP × territoire n'ont rien à faire dans `objects`.
- **Ce qui se rattache à un site devient une propriété du `map_pin`** (le site est l'objet que l'on prospecte) : tension du métier dominant dans son département, offres ouvertes, ancienneté de la plus vieille offre, part d'offres de remplacement, part de seniors du métier dans sa zone d'emploi, score.
- **Les offres deviennent des objets** `job_posting` seulement après rapprochement avec un site (§6), avec un lien daté `posted_at` → `map_pin`. Une offre non rapprochée reste dans le dataset (elle compte pour le territoire, pas pour un lead).
- **Les zones** (département, zone d'emploi) sont des layers `geo-layers` dont les features portent les indicateurs en `props` ; le style colore selon l'indicateur choisi.

## 4 bis. Structures génériques, multi-pays dès la V1

Objectif : qu'une couche pays (`labor_market_de`, `_uk`, `_us`, `_ca`, `_au`, ou `labor_market_eu` pour Eurostat/EURES) ne soit **que des pipelines et des tables de passage**, sans migration, sans nouvelle propriété, sans changement du score ni de la carte.

### Pivots

| Axe | Code local (couche pays) | Pivot (pack générique) | Pourquoi |
|---|---|---|---|
| Métier | FAP2021, ROME (FR) ; KldB (DE) ; SOC (UK, US) ; NOC (CA) ; ANZSCO (AU) | **ISCO-08**, 4 chiffres (repli 3 ou 2 chiffres) | Classification internationale, utilisée par Eurostat et le rapport EURES ; chaque pays publie une table de passage |
| Territoire | département, zone d'emploi, bassin BMO (FR) ; Kreis, Land (DE) ; county, State (US)… | **NUTS** (Europe, niveaux 1–3), sinon `country` + région ISO 3166-2 | Maille commune européenne ; hors Europe, l'ISO 3166-2 donne au moins la région |
| Secteur | NAF (FR), WZ (DE), SIC/NAICS… | **NACE rév. 2** (Europe), NAICS conservé en local (US/CA) | NAF et WZ sont des déclinaisons de NACE ; le registre d'entreprises donne déjà le code local |
| Catégorie sociale | PCS 2020 (FR)… | **ISCO grand groupe** (1 chiffre) + catégorie locale gardée | Pas de classification sociale internationale stable : le grand groupe ISCO en tient lieu |
| Âge | tranches de chaque source | **tranches pivot** `<30`, `30–49`, `50+`, `55+` | Recalculables depuis toutes les sources ; `55+` sert aux départs |
| Période | année, trimestre, édition | `period_start`, `period_end` (dates) + `edition` (texte) | Mêle sans ambiguïté données annuelles, trimestrielles et éditions de rapport |

### Tables de passage (datasets du pack générique, alimentés par chaque couche pays)

| Dataset | Colonnes | Clé |
|---|---|---|
| `lm_occupation_crosswalk` | `country`, `scheme`, `code`, `label`, `isco`, `isco_level`, `weight` (0–1, un code local peut tomber sur plusieurs ISCO) | `(scheme, code, isco)` |
| `lm_area_crosswalk` | `country`, `scheme`, `code`, `label`, `nuts`, `iso_region`, `parent_scheme`, `parent_code`, `weight` | `(scheme, code)` |
| `lm_sector_crosswalk` | `country`, `scheme`, `code`, `nace`, `weight` | `(scheme, code, nace)` |
| `lm_sector_occupations` | `country`, `sector_scheme`, `sector_code`, `isco`, `share` — remplace la table NAF → FAP de M13, générique | `(sector_scheme, sector_code, isco)` |

### Datasets d'indicateurs : une forme longue commune

Toutes les sources statistiques écrivent dans **une seule forme longue**, plutôt qu'une table par source :

| Dataset | Colonnes communes | Colonnes propres |
|---|---|---|
| `lm_indicators` | `country`, `source` (`dares_tension`, `ft_bmo`, `eures_shortage`, `ba_engpass`, `bls_jolts`…), `edition`, `period_start`, `period_end`, `occupation_scheme`, `occupation_code`, `isco`, `area_scheme`, `area_code`, `nuts`, `sector_scheme`, `sector_code`, `metric`, `value`, `unit` | `metric` dans un vocabulaire fermé du pack (ci-dessous) |
| `lm_workforce` | mêmes axes + `age_band`, `sex`, `social_group_scheme`, `social_group_code` | `metric` = `employed`, `seniors_share`… |
| `lm_job_postings` | `country`, `source`, `external_id`, `first_seen`, `last_seen`, `closed_at`, `occupation_scheme`, `occupation_code`, `isco`, `area_scheme`, `area_code`, `lat`, `lon`, `company_name`, `company_registry_id` (SIRET, Handelsregister…, si fourni), `contract`, `motive`, `language`, `title`, `description` | clé `(source, external_id)` |

**Vocabulaire `metric` (pack générique, extensible par version)** : `tension_index`, composantes de tension `hiring_intensity`, `labour_shortage`, `working_conditions`, `geo_mismatch`, `training_mismatch`, `seasonal_projects_pct`, `shortage_flag`, `surplus_flag`, `hiring_projects`, `hiring_difficulty_pct`, `vacancy_rate`, `vacancies`, `hires_rate`, `separations_rate`, `quits_rate`, `retirements_expected`, `replacement_demand`, `employed`, `seniors_share`. Une couche pays qui a besoin d'une nouvelle métrique la propose au pack générique, elle ne l'invente pas localement.

**Comparabilité** : chaque `metric` porte dans le pack sa définition, son sens (« plus haut = plus tendu ») et sa normalisation (rang percentile **dans le pays et l'édition**). Le score ne compare jamais des valeurs brutes de deux pays : il lit le percentile. Un indice DARES et un taux JOLTS ne se confondent pas, mais « métier dans le décile le plus tendu de son pays » se compare.

### Layers

- Une layer par **schéma de maille** : `lm_areas_<area_scheme>` (`lm_areas_fr_dept`, `lm_areas_fr_ze`, `lm_areas_nuts2`, `lm_areas_de_kreis`…), features clées par `area_code`, `props` = dernières valeurs de chaque `metric` par ISCO suivi.
- Le style choisit `metric` et ISCO par expression : le même style sert pour tous les pays. Le `style` d'une layer accepte déjà des couches MapLibre à expressions data-driven (phase A).
- Les contours viennent de la couche pays ; `lm_areas_nuts*` vient du pack générique (contours GISCO/Eurostat), pour la vue Europe.

### Propriétés génériques du site

Les propriétés du `map_pin` (§6.6) sont nommées **sans référence à une source ni à un pays** : `lm_main_occupations` (liste ISCO), `lm_tension_pct` (percentile), `lm_hiring_difficulty_pct`, `lm_open_offers`, `lm_oldest_offer_days`, `lm_replacement_pct`, `lm_seniors_share`, `lm_score`, plus `lm_sources` (source et édition de chaque composante, M14). Le code local du métier reste lisible dans la fiche via la table de passage.

### Ce qu'une couche pays doit fournir (checklist)

1. Pipelines qui écrivent `lm_occupation_crosswalk`, `lm_area_crosswalk`, `lm_sector_crosswalk` pour ses codes.
2. Pipelines d'indicateurs vers `lm_indicators` / `lm_workforce`, avec des `metric` du vocabulaire.
3. Optionnel : un pipeline d'offres vers `lm_job_postings` et ses règles de motif dans **sa langue** (M11 est par langue, pas par pays).
4. Optionnel : contours de ses mailles fines vers `lm_areas_<scheme>`.
5. Fixtures et tests de pipeline. Rien d'autre.

Pistes de couches, non engagées : `labor_market_eu` (rapport EURES pénuries/excédents par ISCO et pays, Eurostat postes vacants et enquête forces de travail, Cedefop besoins de remplacement), `_de` (analyse annuelle des métiers en pénurie de l'agence fédérale pour l'emploi, par Land), `_us` (JOLTS par secteur et État, projections par métier), `_ca` (perspectives à trois ans par métier et région économique), `_au` (liste annuelle des métiers en pénurie par État), `_uk` (postes vacants ONS, liste de pénurie officielle).

## 5. Décisions

| # | Décision | Pourquoi |
|---|---|---|
| M1 | **Deux packs** : `labor_market` (générique : types `occupation`, `job_posting`, propriétés `lm_*` sur `map_pin`, lien `posted_at`, réglages de score, **schémas des datasets et des layers §4 bis**) et `labor_market_fr` (couche France : pipelines S1–S8, codes FAP/ROME/PCS, zones d'emploi, tables de passage vers les pivots). `requires: [companies_fr, labor_market]` | Convention `<pack générique>_<pays>` ; un autre pays se branche en **ajoutant des pipelines qui écrivent dans les mêmes tables**, sans toucher au générique ni au score |
| M2 | **Pipelines uniquement**, planifiés par `recurrence` : S3 quotidien ; S1, S2, S4, S5 annuels ; S6 trimestriel ; S7, S8 à la demande. Aucun flow, aucun `NodeDoc` | Demande de Shan (2026-11-14) ; le moteur D569–D578 sait tout faire sauf les manques de M3–M5 |
| M3 | **Lecture xlsx dans un pipeline** : `read.format: xlsx` (+ `sheet:` nom ou index, `header_row:`), en réutilisant `import_read::sheet_rows` de `pnex-core`. Garde zip-bomb identique à `unpack` | DARES et France Travail publient en xlsx ; les fichiers d'import le lisent déjà, pas les pipelines (`pipeline::doc::Format` n'a pas `Xlsx`) |
| M4 | **Grant OAuth2 client credentials** : `TokenGrant` gagne `kind: password` (défaut, comportement actuel) \| `client_credentials` ; en `client_credentials`, `user`/`password` désignent les secrets envoyés en `client_id`/`client_secret`, `fields` porte `scope` et le reste ; le jeton reste en mémoire le temps d'un passage, jamais stocké | L'API France Travail exige client credentials ; aujourd'hui `token()` envoie toujours `username`/`password` |
| M5 | **Écriture d'un pipeline vers une layer** : `write: { from: <rel>, layer: { key, geometry: <colonne GeoJSON ou WKT>, id: <colonne>, replace: true } }` remplace le contenu de la layer par `services::geo::layers::replace_features` (génération + 1, tuiles nouvelles URL) au lieu d'écrire une table | Les indicateurs par zone changent chaque année : la layer doit se rafraîchir par pipeline, pas par upload manuel ; le remplacement complet existe depuis la phase A |
| M6 | **Prérequis carto = phase A de `geo-layers`, livrée le 2026-11-14** (avancée avant la 0.2.0, décision de Shan) : PostGIS, `geo_layers`/`geo_features`, tuiles MVT, couches sous les sites sur /map. Les phases B–E gardent leur calendrier | Sans polygones, pas de choroplèthe ; `geo_area` (K29) ne suffit pas : un seul anneau, 1 000 sommets, pas de multipolygone (Corse, îles) |
| M7 | **Archive des offres** : dataset `lm_job_postings` en **upsert par `offer_id`** avec `first_seen`, `last_seen`, `closed_at` (posé au premier run où l'offre manque) ; jamais de remplacement complet | Seul moyen de constituer l'historique (durée d'ouverture, republications) que l'API ne donne pas |
| M8 | **Découpage de la collecte S3** par `(département × lot de codes ROME)` via `enumerate`, codes ROME dérivés des métiers ISCO suivis par l'org (`labor_market.occupations`, M18) via `lm_occupation_crosswalk`, `every:` ≥ 150 ms et respect des quotas annoncés | L'API plafonne le nombre de résultats par recherche ; on ne collecte que les métiers qui intéressent l'org |
| M9 | **Rapprochement offre → établissement** en SQL, en trois passes : (1) nom normalisé identique + même commune ; (2) nom proche (trigrammes) + distance < 3 km ; (3) rien → non rapproché. `match_score` 0–1 conservé ; sous un seuil (réglage, défaut 0,8) le lien est créé `to_validate` | Pas de SIRET dans l'offre (à revérifier à LM2) ; même école que les contacts de liste (`org_resolve`, D337) |
| M10 | **Intérim et anonymes** : offre dont l'entreprise a un NAF 78.xx (sociétés d'intérim) ou sans nom → jamais rattachée à un site ; elle alimente les indicateurs du territoire seulement. Le texte peut nommer le client : extraction hors V1 | Sinon l'agence d'intérim devient le « meilleur lead » du département |
| M11 | **Motif de l'offre** (remplacement / départ en retraite / création / renfort / inconnu) par règles de mots-clés en SQL en V1 ; classification LLM (fournisseur LLM d'org, D116) en V2, sur l'offre déjà rapprochée seulement | Les mots-clés couvrent l'essentiel à coût nul ; le LLM coûte, on le garde pour les leads |
| M12 | **Score de site = SQL, poids en réglages d'org** : `score = Σ wᵢ · normᵢ` sur tension, difficulté BMO, effectif, offres ouvertes, ancienneté max d'offre, part de remplacements, part de seniors du métier ; chaque composante est aussi publiée seule sur le `map_pin` | Pas de méthode de vente dans le pack (account-mapping §2) ; chaque chiffre affiché dit sa définition (sales-dashboards §1) |
| M13 | **Métiers d'un site** : dataset générique `lm_sector_occupations` (secteur → ISCO, part), pré-rempli par chaque couche pays (en France depuis BMO « métiers par secteur »), surchargeable par l'org ; un site porte une liste d'ISCO | Aucune source publique ne donne les métiers d'un établissement ; l'approximation par secteur est explicite, corrigeable et indépendante du pays |
| M14 | **Provenance** : chaque propriété `lm_*` écrite sur un objet porte sa source et son millésime (D184) ; la fiche du site affiche « source, année » à côté du chiffre | En rendez-vous, un chiffre sans source ne vaut rien |
| M15 | **Pivots ISCO-08 / NUTS / NACE** dans le pack générique ; les codes locaux restent à côté, jamais à la place (§4 bis) | Ajouter un pays = ajouter des lignes, pas des colonnes ni des propriétés |
| M16 | **Indicateurs en forme longue** (`lm_indicators`, `metric` en vocabulaire fermé) plutôt qu'une table par source | Une nouvelle source n'impose ni migration ni changement du score |
| M17 | **Score sur percentiles par pays et par édition**, jamais sur valeurs brutes | Les indicateurs nationaux ne se comparent pas entre eux ; leur rang dans le pays, si |
| M18 | **Réglages en ISCO** : l'org suit des métiers ISCO (`labor_market.occupations`) ; chaque couche pays en dérive ses codes de collecte (ROME pour l'API Offres en France, via `lm_occupation_crosswalk`) | Un seul réglage pour une org présente dans plusieurs pays |

## 6. Pipelines (pack `labor_market_fr`)

Forme indicative, dans le DSL existant (`enumerate`, `fetch`, `unpack`, `read`, `sql`, `write`). Les noms de colonnes des fichiers sources sont à relever au lot.

### 6.1 `fr_dares_tension` — S1 → `lm_indicators`

```yaml
# Tension indicators per occupation (FAP) and département, DARES / France
# Travail, one edition per year. Columns renamed to English at read.
key: fr_dares_tension
name: Labour market tension (DARES, France)
hosts: [dares.travail-emploi.gouv.fr]
steps:
  - fetch:
      as: file
      url: "{{setting.labor_market_fr.tension_url}}"
  - read:
      as: raw
      from: file
      format: xlsx            # M3
      sheet: "{{setting.labor_market_fr.tension_sheet}}"
      header: auto
      columns:
        - { name: fap, at: "Code FAP", type: text }
        - { name: dept, at: "Département", type: text }
        - { name: tension, at: "Indicateur de tension", type: decimal }
        - { name: hiring_intensity, at: "Intensité d'embauches", type: decimal }
        - { name: labour_shortage, at: "Manque de main-d'oeuvre disponible", type: decimal }
        - { name: working_conditions, at: "Conditions de travail contraignantes", type: decimal }
        - { name: geo_mismatch, at: "Inadéquation géographique", type: decimal }
  # Wide source row -> long generic rows (one per metric), pivots filled
  # from the crosswalks the France layer writes (M15, M16).
  - sql:
      as: long
      tables: { lm_occupation_crosswalk: lm_occupation_crosswalk, lm_area_crosswalk: lm_area_crosswalk }
      sql: |
        WITH m AS (
          SELECT fap, dept, 'tension_index' AS metric, tension AS value FROM raw
          UNION ALL SELECT fap, dept, 'hiring_intensity', hiring_intensity FROM raw
          UNION ALL SELECT fap, dept, 'labour_shortage', labour_shortage FROM raw
          UNION ALL SELECT fap, dept, 'working_conditions', working_conditions FROM raw
          UNION ALL SELECT fap, dept, 'geo_mismatch', geo_mismatch FROM raw)
        SELECT 'FR' AS country, 'dares_tension' AS source,
               '{{setting.labor_market_fr.tension_edition}}' AS edition,
               CAST('{{setting.labor_market_fr.tension_year}}-01-01' AS DATE) AS period_start,
               CAST('{{setting.labor_market_fr.tension_year}}-12-31' AS DATE) AS period_end,
               'fr_fap2021' AS occupation_scheme, m.fap AS occupation_code, o.isco,
               'fr_dept' AS area_scheme, m.dept AS area_code, a.nuts,
               m.metric, m.value, 'index' AS unit
        FROM m
        LEFT JOIN lm_occupation_crosswalk o ON o.scheme = 'fr_fap2021' AND o.code = m.fap
        LEFT JOIN lm_area_crosswalk a ON a.scheme = 'fr_dept' AND a.code = m.dept
        WHERE m.fap IS NOT NULL AND m.value IS NOT NULL
  - write:
      from: long
      table: { name: lm_indicators, key: [source, edition, occupation_scheme, occupation_code, isco, area_scheme, area_code, metric] }
```

### 6.2 `fr_ft_bmo` — S2 → `lm_indicators`

Même forme : `fetch` du fichier de l'année (data.gouv), `read` csv/xlsx, passage en forme longue avec `source = ft_bmo`, `area_scheme = fr_bmo_basin`, métriques `hiring_projects`, `hiring_difficulty_pct` (projets difficiles / projets) et `seasonal_projects_pct`.

### 6.3 `fr_ft_job_postings` — S3 → `lm_job_postings`, quotidien

```yaml
# Active job offers of France Travail for the org's occupations, per
# département; archived by offer id (first/last seen) to build the history
# the API does not keep. Recruiter contact fields are never read.
key: fr_ft_job_postings
name: Job offers (France Travail)
hosts: [entreprise.francetravail.fr, api.francetravail.io]
steps:
  # The org follows ISCO codes (M18); the France layer turns them into the
  # ROME codes the API wants, through the crosswalk it maintains.
  - enumerate:
      as: wanted
      cross:
        - { as: dept, setting: labor_market_fr.departments }
        - { as: isco, setting: labor_market.occupations }
  - sql:
      as: slices
      tables: { lm_occupation_crosswalk: lm_occupation_crosswalk }
      sql: |
        SELECT DISTINCT w.dept, c.code AS rome
        FROM wanted w JOIN lm_occupation_crosswalk c
          ON c.scheme = 'fr_rome' AND c.isco LIKE w.isco || '%'
  - fetch:
      as: answer
      for_each: slices
      url: "https://api.francetravail.io/partenaire/offresdemploi/v2/offres/search"
      params: { departement: "{{dept}}", codeROME: "{{rome}}" }
      token:                     # M4
        kind: client_credentials
        url: "https://entreprise.francetravail.fr/connexion/oauth2/access_token?realm=/partenaire"
        user: lm_ft_client_id
        password: lm_ft_client_secret
        fields: { scope: "api_offresdemploiv2 o2dsoffre" }
      pages: { param: range, style: range, size: 150, until: /resultats }   # range paging to confirm
      every: 200ms
      missing_is_empty: true
  - read:
      as: offers
      from: answer
      format: json
      records: /resultats
      columns:
        - { name: offer_id, at: /id, type: text }
        - { name: rome, at: /romeCode, type: text }
        - { name: title, at: /intitule, type: text }
        - { name: description, at: /description, type: text }
        - { name: company_name, at: /entreprise/nom, type: text, optional: true }
        - { name: commune, at: /lieuTravail/commune, type: text, optional: true }
        - { name: lat, at: /lieuTravail/latitude, type: float, optional: true }
        - { name: lon, at: /lieuTravail/longitude, type: float, optional: true }
        - { name: contract, at: /typeContrat, type: text, optional: true }
        - { name: created_at, at: /dateCreation, type: timestamp }
        - { name: updated_at, at: /dateActualisation, type: timestamp, optional: true }
  - sql:
      as: today
      sql: |
        SELECT 'FR' AS country, 'ft_offers' AS source, offer_id AS external_id,
          'fr_rome' AS occupation_scheme, rome AS occupation_code,
          'fr_commune' AS area_scheme, 'fr' AS language, *, current_date AS seen_on,
          CASE
            WHEN description ILIKE '%départ en retraite%' OR description ILIKE '%depart a la retraite%' THEN 'retirement'
            WHEN description ILIKE '%remplacement%' OR description ILIKE '%suite à un départ%' THEN 'replacement'
            WHEN description ILIKE '%création de poste%' OR description ILIKE '%renfort%' THEN 'growth'
            ELSE 'unknown' END AS motive          -- M11
        FROM offers
  - write:
      from: today
      table: { name: lm_job_postings, key: [source, external_id], upsert: { first_seen: keep, last_seen: seen_on }, close_missing: closed_at }   # M7
```

### 6.4 `lm_matching` — rapprochement (M9, M10), après S3

`sql` qui joint `lm_job_postings` (ouvertes) aux établissements de l'org (`enumerate` sur `map_pin` avec `siret`, `name`, `naf`, position) : passes 1 à 3 de M9, exclusion M10, sortie `lm_offer_matches(source, external_id, registry_id, match_score)` — `registry_id` = SIRET en France, l'identifiant du registre ailleurs ; le rapprochement est générique (nom normalisé, commune, distance), seules la normalisation des formes juridiques et la liste des NACE d'intérim viennent de la couche pays. L'import du pack crée `job_posting` + lien `posted_at` (`to_validate` sous le seuil).

### 6.5 `fr_insee_workforce` et `fr_dares_jobs_2030` — S4, S5

- S4 : `fetch` du zip RP → `unpack` → `read` csv → `sql` qui agrège par **zone d'emploi du lieu de travail** × PCS × tranches d'âge pivot et écrit `lm_workforce` (`area_scheme = fr_ze2020`, `social_group_scheme = fr_pcs2020`, ISCO par la table de passage PCS → ISCO). Le fichier détail est gros : agrégation dans le `sql` du pipeline, seul l'agrégat est écrit (cible Pi).
- S5 : `read` xlsx → `lm_indicators` avec `metric = retirements_expected` et `replacement_demand`, `area_scheme = fr_region`.

### 6.6 `lm_site_signals` — score (M12, M13)

Dernier pipeline, déclenché par l'ordonnanceur à dépendances (D575) après les précédents : un `sql` **générique** (pack `labor_market`, aucune table propre à un pays) joint sites × `lm_sector_occupations` (secteur du site → ISCO) × `lm_indicators` (mailles du site via `lm_area_crosswalk`, percentiles M17) × `lm_workforce` × `lm_offer_matches` ; écrit `lm_site_signals`, dont l'import met à jour les propriétés du `map_pin` :

| Propriété (`map_pin`) | Définition affichée |
|---|---|
| `lm_main_occupations` | Métiers ISCO retenus pour le site (secteur → métiers, M13) ; code local affiché à côté |
| `lm_tension_pct` | Percentile de tension du métier dans la maille du site, dans son pays (en France : indicateur DARES, département) |
| `lm_hiring_difficulty_pct` | Part des recrutements jugés difficiles (en France : BMO, bassin) |
| `lm_open_offers` | Offres ouvertes rattachées au site |
| `lm_oldest_offer_days` | Jours d'ouverture de la plus ancienne offre ouverte |
| `lm_replacement_pct` | Part des offres du site (12 mois) au motif remplacement ou retraite |
| `lm_seniors_share` | Part des 50 ans et plus dans la PCS du métier, zone d'emploi du site (RP) |
| `lm_score` | Somme pondérée (poids de l'org) des composantes en percentiles, 0–100 |
| `lm_sources` | Source et édition de chaque composante (M14) |

### 6.7 `lm_areas` — S8 + indicateurs → layers (M5, M6)

`fetch` des contours → `read` GeoJSON (`format: json`, `records: /features`, colonne `geometry` en json) → `sql` qui joint les indicateurs agrégés par zone (tension moyenne pondérée par métier choisi, difficulté BMO, part de seniors) → `write: { layer: { key: lm_areas_fr_dept … } }` et `lm_areas_fr_ze`. Le `sql` de jointure est générique (il lit `lm_indicators` par `area_scheme`) ; seuls les contours viennent de la couche pays. Une layer par maille ; l'indicateur affiché se choisit par le style (expression data-driven sur `props`).

## 7. Carte

- **Fond** : layer de zones (département ou zone d'emploi), remplissage gradué sur l'indicateur choisi, une palette séquentielle unique ; légende avec la définition et le millésime (M14).
- **Sites** : les `map_pin` de l'org filtrés sur `lm_score` ≥ seuil (défaut : top 50 de la vue), taille selon `headcount_band`, couleur selon `lm_score`, halo si `lm_open_offers > 0`. Clustering D37 inchangé.
- **Filtres** : métier (ISCO, libellé local affiché), maille, seuil de score, « recrute en ce moment ». Un seul métier à la fois par défaut (lisibilité).
- **Fiche du site** : chaque composante du score avec sa source et son année, les offres rattachées (titre, ancienneté, motif) et leur lien `to_validate` éventuel.
- Acquis de la phase A : les couches géographiques se dessinent sous les marqueurs HTML des sites, une case par couche dans le panneau de /map. À vérifier à LM4 : les sites sont des marqueurs HTML (D37) dont seule la couleur de statut (D339) varie ; taille par `headcount_band` et halo demandent un ajout additif au rendu des marqueurs (pas un nouvel éditeur de style : celui de `geo-layers` phase E).

## 8. Sécurité et données personnelles (grille `security.md` §6)

- Secrets France Travail (`lm_ft_client_id`, `lm_ft_client_secret`) au coffre de l'org (D110), envoyés à la seule adresse du grant, hôte déclaré (R8) ; jamais dans un DTO de lecture, une sortie d'outil MCP ou un journal (R4, R16).
- Aucune colonne de contact du recruteur dans `read` (nom, téléphone, e-mail, URL de candidature nominative) : la règle est dans le YAML **et** un test de pipeline vérifie l'absence des colonnes.
- Les données statistiques sont publiques et agrégées ; les données d'entreprise viennent des registres déjà importés. Rien de nouveau ne touche des personnes physiques.
- Org = frontière (R1) : datasets, layers et propriétés `lm_*` sont par org ; une org n'utilise que ses propres identifiants France Travail (licence de l'API).
- Licence de réutilisation des offres (conditions de l'API France Travail) relue avant LM2 : stockage, durée de conservation, affichage.

## 9. Lots

| Lot | Contenu | Fini quand |
|---|---|---|
| **LM0 — Moteur** | M3 (xlsx), M4 (client credentials), pagination `range` si absente, `upsert` + `close_missing` + clé multi-colonnes sur `write.table` (M7, M16) ; vérifier que `fetch.for_each` accepte une relation issue d'un `sql` | Un pipeline de test lit un xlsx et une API à client credentials, fixtures comprises |
| **LM1 — Référentiels et tension** | Packs `labor_market` / `labor_market_fr` ; S7, S1, S2 en datasets ; tables de passage FAP/ROME/PCS → ISCO, départements/ZE/bassins → NUTS, NAF → NACE ; réglage `labor_market.occupations` (ISCO) et dérivation des codes ROME | Tension et BMO interrogeables dans `lm_indicators` par ISCO **et** par code local |
| **LM2 — Offres** | S3 quotidien, archive M7, rapprochement M9–M10, motif M11 (règles) ; `job_posting` + `posted_at` | Après 7 jours : offres ouvertes, ancienneté et motif visibles sur les sites rapprochés ; aucun site d'intérim rattaché |
| **LM3 — Âges et départs** | S4 (agrégé), S5, S6 | Part des seniors par PCS × zone d'emploi et départs 2030 par FAP disponibles |
| **LM4 — Score et carte des sites** | `lm_site_signals`, propriétés `lm_*`, réglages de poids, rendu des sites et fiche | La carte montre les sites scorés d'une région test avec la fiche sourcée |
| **LM5 — Zones** | M5 (écriture de layer par pipeline) + pipeline `lm_areas` ; la phase A de `geo-layers` (M6) est déjà livrée | Le fond départements / zones d'emploi se colore par indicateur, sous les sites |

À chaque lot (conditions de « fini », pas des tâches à part) : clés i18n dans les deux `.ftl` et lexique (`design-system.md` §7 : « Tension », « Offre », « Zone d'emploi »… ; « Couche géographique » y est déjà), codes d'erreur dans `err_codes::ALL`, fiche `assistant-kb` (lecture des indicateurs, définitions), tests de pipeline sur fixtures, revue `security.md` §6, contrat d'API et IAM si une route est ajoutée. Lots à écran (LM4, LM5) : skills de design (frontend-design, emil-design-eng, break-ui).

## 10. Questions ouvertes

1. **Table commune → bassin BMO** : publiée par France Travail avec les résultats, ou à reconstruire ? Sinon, BMO agrégé au département.
2. **SIRET dans l'API Offres** : absent à notre connaissance ; s'il existe pour une partie des offres, la passe 0 de M9 devient « SIRET égal ».
3. **Quotas de l'API Offres** et plafond de résultats par recherche : à relever pour dimensionner M8 sur la France entière.
4. ~~**Phase A de `geo-layers` avant la 0.2.0** (M6)~~ : tranché le 2026-11-14 par Shan, phase A avancée et livrée (PostGIS en dev/CI, image CNPG PostGIS à poser en prod).
5. **Métiers suivis par défaut** : liste vide (l'org choisit ses ISCO) ou une sélection proposée par l'assistant à partir des secteurs des comptes de l'org ; le pack n'embarque aucune liste.
6. **Historique acheté** (agrégateurs commerciaux d'offres) : hors périmètre tant que l'archive M7 n'a pas montré sa valeur.
7. **Tables de passage vers ISCO** : FAP2021 → ISCO n'est pas publiée directement à notre connaissance ; chemin probable FAP → PCS 2020 → ISCO-08 (tables INSEE), avec `weight` < 1 sur les correspondances multiples. À relever à LM1.
8. **Contours NUTS** (GISCO, Eurostat) : licence et format à vérifier pour `lm_areas_nuts*`.

## 11. Avancement (2026-11-15)

Lancé par Shan le 2026-11-15 (« tu peux »), priorité aux besoins de
l'application commerciale : BMO, tension, offres.

**Moteur (LM0), générique, aucun nom de source en Rust** :

- `read.format: xlsx` + `sheet` (nom ou position depuis 1) : la feuille est
  lue par le lecteur de tableur de `pnex-core` (garde zip-bomb), réécrite en
  CSV puis lue comme un CSV (M3).
- `secret.token.kind: client_credentials` : le coffre remplit `client_id` et
  `client_secret` au lieu de `username` et `password` (M4).
- `fetch.json_from` : corps JSON d'un `post` (objet, ou son texte produit par
  une étape `sql`).
- `pages.size` : la pagination par plage d'enregistrements (`range=0-149`,
  `150-299`…), arrêtée par une page incomplète ; un 416 vaut une réponse
  vide, comme un 404.

**Pack `labor_market_fr` 1.0.0**, trois pipelines avec rejeu sur fixtures :

| Pipeline | Table | Rythme | Écart au plan |
|---|---|---|---|
| `fr_bmo` | `lm_bmo` (forme longue, FAP × département) | 24 h, seulement si le fichier change | Bassin non gardé : son nom de colonne change chaque année (`BE26`) ; somme par département |
| `fr_ft_tension` | `lm_tension` (forme longue, ROME × **région**, niveau 1–5, facteurs, libellés) | passes de 30 min, chaque couple suivi (D937) demandé une fois par année civile | **S1 vient de l'API Marché du travail de France Travail**, pas du fichier Dares : le site de la Dares refuse les accès automatisés (choix de Shan, 2026-11-15). Pas de composantes Dares détaillées |
| `fr_ft_offers` | `lm_offers_seen` (une ligne par offre et par jour vu) | 24 h | M7 sans upsert : l'archive est un ajout quotidien, la requête nommée `job_postings` donne premier et dernier jour vus et la clôture. Un test de rejeu ne lit aucune table d'org, d'où ce choix |

- **Réglages** : régions (tension), départements (offres) et **codes ROME** suivis (pas encore ISCO :
  M18 et les tables de passage arrivent avec LM1) ; adresse du fichier BMO.
- **Secrets** : `lm_ft_client_id`, `lm_ft_client_secret` au coffre de l'org
  (application francetravail.io abonnée aux API Offres d'emploi v2 et
  Marché du travail).
- **Lecture** : requêtes nommées `bmo`, `tension`, `job_postings`,
  `company_offers` (`POST /api/v1/analysis/named/labor_market_fr/<clé>`,
  lignes par `GET /api/v1/analysis/queries/{id}`).
- Deux tables d'indicateurs plutôt qu'une `lm_indicators` : deux pipelines
  qui écrivent la même table en remplacement s'effaceraient ; l'union (et
  les pivots ISCO/NUTS) viendra avec LM1.

**Tension par région (1.1.0, 2026-11-15)**, demandée par l'application
commerciale (carte des métiers en tension) : réglage `regions`, territoire
`REG`, scope réduit à l'API Marché du travail (l'API Offres d'emploi v2 a
quitté le catalogue de francetravail.io : la demander fait échouer tout le
jeton en `invalid_scope`). Les ~24 800 couples (13 régions × 1 911 codes
ROME collés dans `rome_codes`) passent en appels séquentiels : 600 par
passe, un jeton par passe (valable 25 min), une quinzaine d'heures au total
une fois par an. Pas de parallélisme au moteur pour une donnée annuelle.
Limite : le jeton n'est jamais renouvelé en cours de passe, d'où `per_run`.
`fr_ft_offers` demande encore le scope Offres v2 : à revoir avec la
deuxième étape (La Bonne Boite).

**Reste** : rapprochement offre → établissement (M9, M10), motif par LLM
(M11), INSEE (S4), départs 2030 (S5), mouvements (S6), score et propriétés
`lm_*` (LM4), couches de zones (LM5, M5). Fixtures France Travail
synthétiques (formes relevées dans le client de l'application commerciale,
documentation de l'API illisible sans navigateur) : à confronter au premier
passage réel.
