# PRD — PNEX Dossiers : enquêter sur données × temps × ontologie × acteurs

**Statut :** Draft v0.1 — 2026-10-11. **C0–C7 livrés sur `main`** (2026-10-11, état en fin de document), puis espace de travail plein écran (`cases-workspace.md`, 2026-10-12).
**Auteur :** Shan
**Périmètre :** un espace de travail borné (« dossier ») qui rassemble, sans
les copier, les objets, données, médias, documents et **acteurs humains**
d'une question à élucider, et les rend lisibles sur un axe temporel, une
carte et un graphe synchronisés.
**Version cible :** à trancher (§11, question 1) — repose sur le noyau
ontologique de la 0.2.0 (`ontology.md`, D176–D191, L0–L5 livrés) ; le gel
des nouveaux piliers s'applique (`roadmap.md` P2.14). Entrée roadmap :
P2.19, décision #24.
**Numérotation :** décisions notées **K1…K19** ici ; numéros D attribués à
la validation (D192+ déjà réservé par `media-vision-studio.md`).
**Renvois :** `ontology.md` (D176 types, D177 identité, D179 liens
temporels, D181 temps des objets, D182 plages, D183 actions, D184
provenance, D185 requête, D186 explorateur, D188 permissions, D189
assistant, D190 packs), `pages.md` (P5 versions, P6 mentions, P13 modèles),
`doc-search.md` (extraction, index, tableurs en SQL), `media-ingest.md`
(D162 segments, D165 transcriptions, D169 plages, D172 garde-fous
juridiques, D173 pas de publication), `geo-layers.md` (traces et couches),
`notifications.md` (D86 journal O2), `annotations.md`, `ai-assistant.md`
§9 (D123, D142–D145), `security.md` (R1–R20).

> **2026-10-12 — refonte de l'écran :** `cases-workspace.md` (v0.2,
> K20–K41) remplace l'écran décrit ici (K6 pour le rendu, K14) et amende
> K1 (deux tables : propositions et gels), K2 (membres déduits des
> attaches d'événements, `in_case` = événements et bac) et K6 (signaux
> bruts séparés des événements retenus). Le modèle d'enquête de ce
> document (pack, affirmations, apparitions, voix) reste valable.

---

## 1. Constat

Le noyau 0.2.0 sait dire **ce qui existe** (objets typés), **comment c'est
relié et depuis quand** (liens temporels) et **d'où vient chaque fait**
(provenance). Il ne sait pas encore répondre à la question que se posent
tous ceux qui ouvrent PNEX après un problème :

> **Que s'est-il passé, dans quel ordre, qui était impliqué, et qu'est-ce
> qui le prouve ?**

Aujourd'hui, pour y répondre, l'utilisateur ouvre six pages (devices,
dashboards, journal des alertes, médias, carte, explorateur d'objets),
recolle les horodatages à la main et rédige sa conclusion ailleurs. Le
**contexte de travail** — la liste des choses qui comptent pour *cette*
question, la fenêtre de temps qui l'intéresse, ses hypothèses — n'existe
nulle part.

Et l'ontologie actuelle parle surtout de choses (pompes, capteurs, POI) :
les **personnes et organisations** n'y sont que des types possibles d'un
pack (Couverture médiatique, D190), sans rôle, sans affirmation sourcée,
sans incertitude de date.

## 2. Vision

> **Un dossier est une lentille sur l'ontologie : il désigne ce qui
> compte pour une question, sur une fenêtre de temps, et le montre sur
> une seule chronologie, une carte et un graphe synchronisés — chaque
> élément renvoyant à sa source.**

Trois principes :

1. **Le dossier ne copie rien.** Il référence des objets existants (un
   device, un flux, un média, une page, un document, une personne) par
   des liens. Une donnée appartient toujours à l'org, jamais au dossier.
2. **Le temps est l'axe principal.** Tout ce qui est daté — mesure,
   alerte, transcription, photo, changement d'objet, ouverture d'un lien,
   événement saisi à la main — se range sur la même chronologie.
3. **Le fait et sa preuve sont séparés.** Une affirmation (« X était à Y
   le 3 mars ») est un objet ; ce qui la soutient (une page de PDF, une
   minute de transcription, une fenêtre de série) en est un autre ; le
   lien entre les deux porte l'emplacement exact.

### Nommage

| Candidat | Pour | Contre |
|---|---|---|
| **Dossier** / **Case** ✅ | Neutre et juste pour les quatre personas (dossier d'enquête, dossier d'incident, dossier de vidéo, dossier de presse) ; mot courant en FR et EN | — |
| Enquête / Investigation | Parlant pour journaliste et enquêteur | Trop policier pour une RCA industrielle ou un maker |
| Projet | Familier | Déjà pris (projets firmware D87) ; évoque la gestion de projet |
| Espace de travail | Générique | Confusion avec l'org ; ne dit pas qu'il y a une question à élucider |

Proposé : **« Dossiers »** dans l'UI fr-FR, **« Cases »** en en-US, type
système `case`. Le mot « enquête » reste disponible comme modèle de
dossier (§K13).

## 3. Utilisateurs et cas d'usage

| Persona | Cas d'usage vedette |
|---|---|
| **Ingénieur IoT / maintenance (RCA)** | Une alerte « pression basse P-12 » à 10:02. Il ouvre un dossier **depuis l'alerte** : pompe, capteurs, ligne et fenêtre ±2 h pré-remplis. La chronologie montre l'ordre réel : action opérateur 09:58, chute de pression 10:01, alerte 10:02, remplacement du capteur la veille (lien `measures` rouvert). Il pose deux hypothèses, rattache les preuves, conclut dans une page « 5 pourquoi » |
| **Enquêteur « données seules »** | Importe des relevés (CSV, xlsx), des traces GPS et des documents. Chaque ligne devient un événement daté et localisé ; les numéros, plaques et noms deviennent des objets. Il travaille sur la chronologie croisée et le graphe de liens à date, sans jamais toucher à un device |
| **Vidéaste true-crime** | Organise une affaire publique : personnes, lieux, dates (souvent approximatives), sources (articles, documents d'audience, extraits vidéo). Chaque affirmation de son script a sa source et son statut (corroboré, contesté) ; il exporte la chronologie pour son montage |
| **Journaliste d'investigation** | Croise flux média transcrits (P2.13), documents, données et acteurs : qui possède quoi depuis quand (liens temporels), qui a dit quoi où (segments sourcés), quelles affirmations se contredisent. Ses sources sensibles restent **confinées** (§K9) |

## 4. Non-objectifs (V1)

- Un outil de gestion de projet (tâches, sprints, Gantt) : les tâches
  d'un dossier sont des procédures de page (`pages.md` P9).
- Un moteur d'analyse de liens à la i2 Analyst's Notebook complet (motifs
  de graphe, analyse de réseaux sociaux avancée) : V1 = graphe à date,
  chemins et voisinage (déjà dans l'interface graphe A3).
- La reconnaissance faciale ou l'identification de personnes à partir
  d'images (`media-vision-studio.md` reste sur objets et postes).
- La collecte OSINT automatisée sur les réseaux sociaux ou le web
  (scraping de profils) : les sources entrent par import, flux déclarés
  (D159) ou `http-fetch`, jamais par un collecteur de personnes.
  Réexamen en cours (2026-10-18) : `extension-collector.md` D254 propose
  le suivi de sources publiques **déclarées une par une** par des agents
  navigateur, sans découverte ni parcours de profils.
- La publication d'un dossier sur le web (D173 : rien ne sort de l'org
  autrement que par un export volontaire).
- La fédération de dossiers entre orgs.

## 5. Décisions proposées

### K1 — Le dossier est un type système de l'ontologie

- Type système `case` (identité universelle D177, objet temporel comme
  les autres), propriétés : `title`, `question` (la question à
  élucider, texte court), `status` (`open`, `concluded`, `archived`),
  `window_from` / `window_to` (fenêtre d'intérêt, optionnelle et
  ouverte), `area` (emprise géographique optionnelle), `template`
  (§K13).
- Il hérite de tout le noyau : page objet, provenance, recherche,
  assistant, export YAML. **Aucune table parallèle** : seul ce qui n'est
  pas exprimable en objets et liens justifie une table (aucune prévue en
  V1).
- Un dossier peut contenir des sous-dossiers par containment (D180) —
  une affaire, ses volets.

### K2 — L'appartenance est un lien `in_case`, temporel et attribué

- Type de lien système `in_case` : *n'importe quel objet* → `case`,
  attributs `role` (`subject`, `actor`, `evidence`, `context`,
  `hypothesis`, `note`), `pinned` (bool), `note` (texte court).
- La validité temporelle de D179 sert telle quelle : retirer un objet du
  dossier **ferme** le lien, il ne le supprime pas — on sait ce que le
  dossier contenait à une date.
- Ajout depuis partout : bouton « Ajouter à un dossier » sur la page
  objet générique et sur les vues spécialisées (device, média, flux,
  POI, page, alerte du journal).
- **Élargissement assisté** : depuis un membre, « ajouter le voisinage »
  propose les objets à ≤ 2 sauts (interface graphe A3) valides dans la
  fenêtre ; l'utilisateur coche, rien n'est ajouté d'office.

### K3 — Ouvrir un dossier depuis un événement

Le geste RCA : sur une alerte (journal D86), un événement caméra, un
segment de transcription ou un point de courbe, **« Ouvrir un
dossier »** crée le dossier avec :

- la fenêtre `t ± marge` (marge par défaut 2 h, réglable) ;
- l'objet source, ses objets liés à 1 saut valides à `t` (pompe, ligne,
  capteurs liés par `measures`, POI de placement) ;
- l'événement déclencheur comme premier élément de la chronologie.

### K4 — Pack « Enquête » : acteurs, événements, pièces, affirmations

Règle D190 respectée : **le noyau ne gagne que `case`, `in_case` et la
chronologie (K6)** ; le métier vit dans un pack installable `investigation`.

| Type | Rôle | Propriétés clés |
|---|---|---|
| `person` | Acteur humain | `name`, `aliases[]`, `role_label`, `notes` |
| `organization` | Personne morale, groupe, service | `name`, `kind`, `identifiers[]` |
| `event` | Quelque chose qui s'est passé | `occurred` (temps flou, K5), `place` (lien POI ou coordonnées), `summary` |
| `piece` | Pièce : enveloppe d'une source | cible (média, document, segment, série, page, URL archivée), `captured_at`, `sha256` (K12) |
| `claim` | Affirmation | `statement`, `status` (K7), `credibility` (K7) |
| `hypothesis` | Hypothèse / piste | `statement`, `status` (`open`, `supported`, `refuted`, `retained`) |
| `identifier` | Numéro, plaque, compte, IMEI, adresse mail | `kind`, `value` (normalisée) |

Liens du pack : `involves` (event → person|organization|objet, attribut
`role` libre : témoin, opérateur, propriétaire…), `owns`, `member_of`,
`employs`, `knows` (temporels, D179), `uses_identifier`, `about`
(claim → n'importe quel objet), `sourced_by` (claim → piece, attribut
`locator`, K7), `supports` / `contradicts` (claim → hypothesis | claim),
`occurred_at` (event → POI).

`person` et `organization` sont **partagés** avec le pack Couverture
médiatique (Repère) : un type, deux packs qui le déclarent en dépendance
(le format pack gagne `requires`, aligné avec `hub.md`).

Les objets d'IoT ne sont pas des acteurs de seconde zone : un `event`
peut `involves` une pompe ou un device au même titre qu'une personne.

### K5 — Le temps flou est une valeur de première classe

Les journalistes et vidéastes datent « courant mars 2019 », « avant le
procès », « entre 21 h et 23 h ». Un horodatage précis inventé est une
erreur de dossier.

- Nouveau type de propriété `fuzzy_time` dans le format maison (§10.2 de
  `ontology.md`) : `{from, to, precision}` avec `precision` ∈ `instant`,
  `minute`, `hour`, `day`, `month`, `year`, `range`, `approx`.
  `instant` impose `from == to` ; `approx` impose une borne de confiance.
- La chronologie dessine un intervalle (barre) et non un point quand la
  précision l'exige ; le tri utilise `from`, l'égalité est dite
  « simultanée à la précision près ».
- Contraintes d'ordre relatives (« A avant B ») : lien `before`
  (event → event), vérifié contre les intervalles ; une incohérence est
  signalée, pas bloquée.

### K6 — Une chronologie unifiée, primitive du noyau

`POST /api/v1/cases/{id}/timeline` (et `POST /api/v1/timeline` sur une
sélection libre d'objets, pour réutilisation hors dossier) fusionne, sur
la fenêtre demandée et pour les membres du dossier :

| Source | Origine | Rendu |
|---|---|---|
| Événements `event` | objets (K4, K5) | point ou barre, couloir des acteurs impliqués |
| Ouvertures / fermetures de liens | `resource_edges` (D179) | « capteur C47 → C88 », « X devient dirigeant de Y » |
| Changements d'objets | O2 `object_changes` (D184) | discret, filtrable |
| Alertes et notifications | O2 journal (D86) | marqueur coloré par sévérité |
| Événements caméra / vision | `event-log` (D73–D86) | vignette |
| Segments transcrits | O2 `tx_<slug>` (D165) | extrait court, lien vers la minute |
| Plages | `time_ranges` (D182) | bande de fond |
| Apparitions média | O2 `appearances` (K17) | marqueur dans le couloir de la personne, lien vers la minute |
| Médias datés | médiathèque (`captured_at`) | vignette |
| Séries | propriétés `series` des membres (D181) | sparkline dans le couloir de l'objet, agrégée (D182) |

- Couloirs (swimlanes) par membre épinglé ou par type ; les autres
  éléments vont dans un couloir « dossier ».
- Pagination par curseur temporel, plafonds par source ; les séries ne
  descendent jamais en points bruts au-delà d'une densité (agrégat O2).
- Lecture via le point unique D185 : le filtre d'ACL futur (K9) s'y
  branche sans toucher la chronologie.
- La chronologie **n'écrit rien** : elle se calcule. Un élément qu'on
  veut retenir s'épingle (lien `in_case`, rôle `evidence`) ou devient
  une `piece`.

### K7 — Affirmations sourcées, statut et crédibilité

- `claim.status` ∈ `unverified`, `corroborated`, `contested`, `refuted`.
  Le passage à `corroborated` exige ≥ 2 liens `sourced_by` vers des
  pièces **d'origines distinctes** (contrôle serveur, contournable avec
  justification tracée).
- `claim.credibility` : grille optionnelle à deux axes, fiabilité de la
  source (A–F) × crédibilité de l'information (1–6), standard des métiers
  du renseignement et du fact-checking ; désactivable par dossier.
- `sourced_by.locator` désigne l'endroit exact : page d'un document
  (`doc-search.md`), horodatage d'un média, id de segment, fenêtre d'une
  série, cellule d'un tableur importé. Cliquer ouvre la source **à cet
  endroit**.
- Distinction tenue avec D184 : la provenance dit **comment le fait est
  entré dans PNEX** ; `sourced_by` dit **ce qui le prouve dans le monde**.
  Les deux s'affichent dans l'onglet Provenance.
- Vue « faits non sourcés » et « affirmations contradictoires » du
  dossier.

### K8 — Hypothèses et modèles RCA

- Tableau des hypothèses : chaque `hypothesis` avec ses affirmations
  `supports` / `contradicts`, leur statut, et un solde lisible (pas de
  score automatique en V1 — l'humain conclut).
- Modèles de pages (`pages.md` P13) fournis par le pack : « 5 pourquoi »,
  « Ishikawa (6M) », « Compte rendu d'incident », « Note d'enquête »,
  « Script / conducteur » (vidéaste). Une page de dossier est liée
  `in_case` et peut mentionner tout membre (P6).
- La conclusion d'un dossier = une page désignée (`conclusion_page`),
  versionnée nommée au passage en `concluded`.

### K9 — Confinement : V1 par l'org, V2 par le dossier

Le modèle de menace (`security.md`) fait de l'**org** la frontière ;
D188 repousse les ACL par objet ; `pages.md` refuse les pages privées.
Le dossier ne crée **pas** de frontière de sécurité en V1 :

- **Dossier ouvert** (défaut) : visible des membres de l'org, droits du
  rôle d'org + verrou par type (D188). C'est le cas RCA : l'équipe voit
  le dossier d'incident.
- **Dossier confiné** : vit dans une **org dédiée** (l'org perso ou une
  org créée pour l'affaire). C'est déjà une frontière éprouvée (R1) ;
  aucun code de sécurité nouveau. Le parcours « Nouveau dossier
  confiné » crée l'org et y invite, en un geste.
- **V2 (décision explicite, question 3)** : ACL par dossier — le premier
  ACL par objet de PNEX, branché sur le point unique D185 (lecture) et
  sur les services d'écriture (R2). Un membre d'un dossier restreint ne
  voit que les objets `in_case`, le reste de l'org lui est opaque. À ne
  pas commencer avant que D188 soit rouvert.
- Journal de consultation (qui a lu quoi) pour les dossiers confinés :
  stream O2 `case_reads`, opt-in par dossier.

### K10 — Imports « données seules »

- **Tableurs et CSV** (`doc-search.md`, Parquet + DataFusion) : un
  assistant de mapping colonne → `event` (date, précision, lieu,
  acteurs) ou → objets (`person`, `identifier`), avec aperçu, déduplication
  par valeur normalisée et `source_ref = import:<job>` (D184). Chaque
  objet créé garde un `sourced_by` vers la ligne d'origine (locator =
  feuille + ligne).
- **Traces GPS** (GPX, GeoJSON, KML) : via `geo-layers.md` ; chaque trace
  liée à un acteur produit des événements de position consultables sur
  la carte du dossier et des **co-présences** calculées (deux acteurs à
  moins de *d* mètres pendant *t* minutes), proposées comme événements.
- **Documents et médias** : médiathèque existante ; une `piece` est créée
  à l'ajout au dossier, avec hash (K12).

### K11 — Extraction proposée, jamais imposée

- Sur un document, une transcription ou un tableau importé, l'assistant
  (LLM de l'org, D119) **propose** des entités (personnes,
  organisations, dates, lieux, identifiants) et des affirmations, avec
  leur `locator`.
- Les propositions arrivent dans une **file de revue** du dossier :
  accepter crée l'objet (`source_ref = assistant:<user>`, l'humain qui
  accepte) et rapproche d'un objet existant si la valeur normalisée
  correspond ; refuser l'écarte. Rien n'entre dans l'ontologie sans
  geste humain (D123, D189).
- Pas d'inférence sur les personnes au-delà du texte (ni âge, ni
  origine, ni opinion déduits).

### K12 — Intégrité des pièces et gel du dossier

- Chaque `piece` porte le `sha256` de son contenu au moment de l'ajout ;
  une divergence ultérieure (fichier remplacé) est affichée, l'ancienne
  version reste désignée.
- **Gel** : une version nommée du dossier (école `pages.md` P5) fige la
  liste des membres (liens `in_case` à date), les statuts des
  affirmations et la page de conclusion. « État du dossier au 12/10 »
  se relit à l'identique grâce à la validité temporelle des liens.
- Ce n'est pas une chaîne de possession à valeur légale (pas
  d'horodatage qualifié en V1) : le PRD le dit, l'UI aussi.

### K13 — Modèles de dossier

Un modèle = pack (D190) qui préconfigure un dossier : types attendus,
rôles, couloirs de chronologie, pages modèles, marge par défaut.

| Modèle | Préréglage |
|---|---|
| **Incident / RCA** | ouverture depuis une alerte (K3), couloirs = équipements, pages 5 pourquoi + Ishikawa, crédibilité désactivée |
| **Enquête** | acteurs, identifiants, imports tabulaires, crédibilité activée, proposition de confinement (K9) |
| **Affaire publique** (vidéaste) | temps flou par défaut, script / conducteur, export chronologie |
| **Investigation média** | flux transcrits, plages d'émission, pack Couverture médiatique requis |

### K14 — Vues synchronisées : chronologie, carte, graphe

- Une sélection de fenêtre sur la chronologie filtre la carte (positions,
  événements localisés, POI membres) et règle `as_of` du graphe (liens
  valides à la date) ; cliquer un nœud du graphe surligne son couloir.
- Lecture « au curseur » : déplacer le curseur temporel rejoue l'état
  des liens et des positions (pas d'animation vidéo en V1).
- Toutes les vues sont des composants existants (vue graphe L4,
  `pnexMap`, widgets de série) ; seul le panneau chronologie est neuf.

### K15 — Assistant scopé au dossier

- Une conversation peut être **attachée** à un dossier : ses outils de
  lecture sont filtrés aux membres et à la fenêtre (paramètre `case_id`
  sur `query_ontology`, `search_documents`, chronologie).
- Nouveaux outils (règle §9.3 de `ai-assistant.md`) : `case_timeline`
  (lecture), `propose_case_members`, `propose_claims` (vers la file de
  revue K11). Aucun outil n'ajoute ni ne retire un membre sans revue ;
  aucun outil ne change le statut d'une affirmation.
- Gestes visés : « résume la chronologie entre 09:00 et 11:00 »,
  « quelles affirmations n'ont qu'une source ? », « qu'est-ce qui a
  changé sur P-12 dans les 24 h avant l'alerte ? ».
- Fiches `assistant-kb/cases.md` et entrée par type système dès le lot
  C0 (règle CLAUDE.md).

### K16 — Garde-fous données personnelles

Un dossier qui nomme des personnes traite des données personnelles,
parfois sensibles. Dans la lignée de D172 :

- Rétention par dossier : à l'archivage, proposition de purge des objets
  `person` / `identifier` qui ne sont membres d'aucun autre dossier ni
  d'aucun autre lien.
- Mention dans la page du dossier de la base de traitement déclarée
  (champ libre : intérêt légitime, exercice journalistique — art. 85
  RGPD, mission, consentement) ; modèle de registre dans la doc
  d'installation.
- Pas de champs « suspect », « coupable » dans le pack : un acteur a des
  rôles dans des événements, la qualification juridique n'est pas un
  attribut d'objet.
- Export (K12, §7) : avertissement explicite quand un export contient
  des `person`.

### K17 — Apparitions média : qui était présent, où, quand (live et différé)

Cas vedette du journaliste et de Repère : une émission politique captée
en continu (`media-ingest.md`, D159–D175). Le dossier doit savoir **qui
est apparu dans quel média, à quelle heure de diffusion, et ce qu'il y a
dit** — sans changer le concept : une apparition est un fait daté et
sourcé comme un autre.

- **Deux horloges, toujours tenues** :
  - `aired_at` : l'heure **de diffusion** (horloge murale). En live, c'est
    l'horloge des segments (D160 : `EXT-X-PROGRAM-DATE-TIME`, horloge du
    porteur). En différé (replay, podcast, fichier importé), c'est la date
    de première diffusion, lue dans les métadonnées, la grille (D169,
    `planned_*`/`actual_*`) ou saisie ; sa précision est un `fuzzy_time`
    (K5) quand elle n'est connue qu'au jour.
  - `media_offset_ms` : la position dans l'enregistrement (segment ou
    média de la médiathèque). C'est ce qui permet d'ouvrir la source
    **à la minute** (locator K7).
  - Chronologie : un élément se place à `aired_at + offset`. Un média
    rediffusé a plusieurs `aired_at` (une plage par diffusion), une seule
    position.
- **Deux grains de stockage** (école D165 : le volume va dans O2, pas dans
  Postgres) :
  - **Événement d'apparition** (grain segment ou phrase) → O2 stream
    `appearances` : `{person_id, stream|media_asset, range_id?,
    aired_at, media_offset_ms, duration_ms, kind, speaker_label?,
    segment_id?, origin, confidence, source_ref}`. Lu par la chronologie
    (K6, nouvelle source) et par l'assistant.
  - **Lien d'ontologie** (grain émission) → `appears_in` (person →
    `time_range` | `media_asset` | flux), temporel (validité = la plage),
    attributs `kind`, `origin`, `confidence`. Un lien par personne et par
    émission, jamais par segment.
- **`kind`** : `present` (en plateau, au micro), `speaking` (temps de
  parole attribué), `mentioned` (nommée dans la transcription, par les
  alias de l'objet, D168). « Mentionné » n'est jamais « présent ».
- **Comment une personne est rattachée** — jamais par la voix ni le
  visage (non-objectif de `media-ingest.md` §2, §4 ici) :
  1. **Annonce** : invités de la grille (`time_ranges.attrs`), EPG, page
     d'émission récupérée par `http_fetch` → `present` sur la plage,
     `origin = grid`.
  2. **Transcription** : auto-présentation ou présentation par
     l'animateur (« je reçois X ») détectée dans le texte → candidat
     « l'étiquette locale `S3` de ce segment = X », `origin = transcript`.
     L'étiquette de diarisation reste locale (S1..S16, `media-ingest.md`
     lot 5) : le rattachement vaut **pour la plage**, pas pour la voix.
  3. **Mention** par les alias → `mentioned`, `origin = alias`.
  4. **Bandeaux** (OCR, tranche ultérieure `media-ingest.md` §13) →
     `origin = chyron`.
  5. **Manuel** depuis la transcription (sélection d'un passage →
     « c'est X »), `origin = manual`.
- **Qui écrit** : un **flow déployé par l'humain** (D123) via un nœud
  `appearance_upsert` (idempotent : clé personne + plage + kind),
  alimenté par `media_source` et `range_upsert` ; le LLM de l'org peut y
  servir de **classifieur fermé** (D168 : sortie = un id de personne
  connu ou `none`, texte passé comme donnée non fiable). Un rattachement
  sous un seuil de confiance (réglage du nœud) part en **file de revue**
  (K11) au lieu d'être écrit. L'assistant, lui, ne fait que proposer.
- **Temps de parole par personne** : quand `S3` est rattaché à X pour une
  plage, la série `media_speaker_seconds{speaker=S3}` de cette plage se
  relit en temps de parole de X à la requête (jointure apparition ×
  série, pas de nouvelle série à label `person` : cardinalité bornée).
- **L'IA est au courant** : outils de lecture `media_appearances`
  (« qui est passé sur France Inter cette semaine ? », « où X a-t-il
  parlé de la réforme ? » → segments `tx_<slug>` cités avec locator) et
  source `appearances` dans `case_timeline` ; fiche `assistant-kb`.

### K18 — Collecte pilotée par l'IA, exécutée par des flows

La collecte (quels flux capter, quelles grilles relever, quelles
personnes suivre, quoi enregistrer dans O2) sera surtout **conçue par
l'assistant**. La règle D123 / D142–D145 ne bouge pas : l'IA conçoit,
l'humain active.

- L'assistant peut, via les services partagés avec l'UI : proposer un
  flux (`media_streams` en brouillon, inactif), écrire un **brouillon de
  flow** de collecte (grille `http_fetch` → `range_upsert`,
  `media_source` → `appearance_upsert`), créer des `person` avec alias
  **en file de revue**, ajouter au dossier des propositions de membres.
- Il ne peut pas : activer une capture, déployer un flow, accepter une
  proposition, écrire directement dans O2.
- Un dossier « Investigation média » (K13) peut porter une **consigne de
  veille** (texte : personnes, thèmes, chaînes) : l'assistant attaché au
  dossier s'en sert pour proposer flux, flows et alias ; chaque
  proposition acceptée garde `source_ref = assistant:<user>` (D184).

### K19 — Identification vocale par enrôlement consenti (à trancher, avis juridique requis)

> **2026-10-11 : variante A implémentée** comme domaine séparé, voir
> `voice-id.md` (V1–V10). Écart assumé : l'audio d'enrôlement est conservé
> en médiathèque (choix produit), l'audio testé ne l'est pas. Variante B
> non retenue. L'avis juridique reste à obtenir avant un usage réel.

Demande produit (2026-10-11) : pré-enregistrer la voix des personnes
pour nommer directement les locuteurs. Techniquement direct : les
empreintes de locuteur existent déjà (`media-ingest.md` lot 5,
`SpeakerLinker`, en mémoire seulement) ; les comparer à des voix de
référence nommerait `S3` sans passer par la grille ni la transcription.

**Cadre juridique (rappel, pas un avis juridique) :**

- Une empreinte vocale traitée pour **identifier** une personne est une
  donnée biométrique (RGPD art. 4.14) : interdite par défaut (art. 9.1).
  Bases possibles : **consentement explicite** (9.2.a) ; dérogation
  journalistique nationale (art. 85 RGPD, art. 80 de la loi
  Informatique et Libertés en France, portée exacte à confirmer). Une
  personnalité qui parle à l'antenne n'a pas pour autant « rendu
  publique » son empreinte (9.2.e ne s'applique pas d'évidence).
- **AI Act** : l'identification biométrique à distance (sans
  participation active de la personne, par comparaison à une base de
  référence) est un système **à haut risque** (annexe III, point 1.a) ;
  PNEX en serait le fournisseur, et l'exemption open source ne couvre
  pas le haut risque (gestion des risques, documentation technique,
  journalisation, contrôle humain, marquage). Calendrier d'application
  à vérifier au moment de décider.
- C'est aujourd'hui un **non-objectif explicite** (`media-ingest.md` §2,
  §4 ici) : le lever demande une décision D explicite.

**Seule forme envisagée si la décision est prise :**

- Désactivé par défaut, activé par l'admin plateforme **et** l'owner de
  l'org, avec la base de traitement déclarée (K16).
- **Enrôlement consenti uniquement** : une voix de référence n'existe que
  pour une `person` qui a donné un consentement explicite, enregistré
  (date, portée, preuve), révocable ; révocation = suppression immédiate
  du gabarit. Aucun enrôlement à partir d'un extrait d'émission d'une
  personne qui n'a pas consenti.
- Gabarit = vecteur seul (jamais l'audio d'enrôlement conservé), chiffré
  au coffre (D110–D118), par org, jamais exporté, jamais lu par
  l'assistant ni par un flow.
- Une correspondance produit une **proposition** d'apparition
  (`origin = voice`, score), en file de revue (K11) : jamais écrite
  d'office, toujours contrôle humain.
- Journal d'usage (O2) : chaque comparaison, chaque correspondance.

**Variante B — enrôlement depuis des sources publiques (personnalités
publiques sans consentement)**, demandée le 2026-10-11 (« leur voix est
disponible partout ») :

- Disponible ≠ licite : la CNIL a sanctionné Clearview AI (20 M€, 2022)
  pour une base biométrique construite à partir d'images publiques ; une
  base de voix tirée de l'antenne est le même cas.
- Seule base plausible : la **dérogation journalistique** (art. 80 LIL),
  pour une org qui exerce réellement le journalisme, limitée aux
  personnalités publiques dans leur rôle public, déclarée dans le
  dossier (K16). Le volet AI Act haut risque reste entier.
- Si retenue : réservée aux orgs déclarées « presse » par l'admin
  plateforme, mêmes garde-fous que ci-dessus (gabarit seul, coffre,
  propositions en revue, journal), plus un registre des gabarits
  consultable par l'org et une procédure d'effacement sur demande.

## 6. Modèle de données (récapitulatif)

| Élément | Où | Nouveau ? |
|---|---|---|
| `case` | type système, table `objects` + identité universelle | type système |
| `in_case` | type de lien système, `resource_edges` | type de lien système |
| `fuzzy_time` | type de propriété du format maison (`pnex-core`) | oui, noyau |
| Pack `investigation` | `crates/pnex-backend/packs/investigation.yaml` | pack |
| `requires` dans le format pack | `pnex-core` (aligné `hub.md`) | extension |
| File de revue K11 | objets `proposal` du pack (statut, cible, locator) | pack |
| O2 `case_reads` | journal de consultation, opt-in | stream |
| `appears_in` | lien du pack (person → plage, média, flux), K17 | type de lien (pack) |
| O2 `appearances` | événements d'apparition au grain segment, K17 | stream |
| Nœud `appearance_upsert` | runtime de flows, `NodeDoc` | nœud |

Aucune nouvelle table Postgres prévue en V1.

## 7. API (additive)

- `POST /api/v1/cases` (création, y compris depuis un événement : corps
  `from = {kind, ref, at}`) ; le reste du CRUD passe par
  `/api/v1/ontology/*`.
- `POST /api/v1/cases/{id}/members` / `DELETE …/members/{object_id}`
  (ouvre / ferme `in_case`), `POST /api/v1/cases/{id}/neighbourhood`
  (propositions K2).
- `POST /api/v1/cases/{id}/timeline`, `POST /api/v1/timeline`.
- `POST /api/v1/cases/{id}/imports` (job `case_import`, mapping K10).
- `GET /api/v1/cases/{id}/proposals`, `POST …/proposals/{id}/accept|reject`.
- `POST /api/v1/cases/{id}/freeze` (version nommée K12),
  `GET /api/v1/cases/{id}/export` (YAML + manifeste des pièces + CSV de
  chronologie + page de conclusion).

`CONTRACT` inchangé : routes additives. Chaque route d'écriture : rôle en
tête de handler + test viewer → 403 (R2), org depuis le principal (R1).

## 8. Lots

| Lot | Contenu | Sortie vérifiable |
|---|---|---|
| **C0** | K1, K2 : type `case`, lien `in_case`, page dossier (membres, fenêtre), « Ajouter à un dossier » partout, fiche assistant | dossier de 10 objets hétérogènes (device, média, page, POI), retrait = lien fermé |
| **C1** | K6 (sources objets, liens, changements, alertes, plages, séries) + panneau chronologie à couloirs | RCA P-12 rejouée : ordre action → chute → alerte → remplacement capteur |
| **C2** | K3 ouverture depuis une alerte / un point de courbe, K13 modèle Incident | dossier d'incident en un clic depuis le journal |
| **C3** | K4, K5, K7 : pack `investigation`, temps flou, affirmations sourcées, vue « non sourcé » | affaire de démo : 20 événements flous, 30 affirmations, 2 contradictions détectées |
| **C4** | K14 : synchro chronologie × carte × graphe `as_of` | curseur temporel qui rejoue liens et positions |
| **C5** | K10 imports tabulaires et GPS, co-présences | 10 000 lignes CSV → événements, dédup d'identifiants |
| **C6** | K11, K15 : extraction proposée, file de revue, assistant scopé | entités d'un PDF de 40 pages proposées avec locator, acceptées une à une |
| **C7** | K8, K12, K16 : hypothèses, modèles RCA, gel, export, rétention | dossier gelé relu à l'identique ; export complet |
| **C8** | K17, K18 : apparitions (grille, transcription, mention, manuel), nœud `appearance_upsert`, source `appearances` de la chronologie, outil `media_appearances`, brouillons de collecte par l'assistant | émission live captée : invités rattachés depuis la grille, temps de parole par personne ; même émission en replay placée à son heure de diffusion |
| **V2** | K9 ACL par dossier (après réouverture de D188) | membre restreint qui ne voit que le dossier |

## 9. Critères de succès

- Une RCA d'incident réelle sur le banc (P0.1) se mène **sans quitter
  le dossier** : de l'alerte à la page de conclusion.
- Les quatre personas de §3 ont un dossier de démo reproductible
  (fixtures E2E), dont un « données seules » sans aucun device.
- Chaque élément affiché dans la chronologie ouvre sa source à
  l'endroit exact (zéro élément orphelin, test E2E).
- Chronologie d'un dossier de 200 membres sur 30 jours < 1 s au p95 sur
  la stack Pi de référence (séries agrégées).

## 10. Risques

- **Dispersion** (gel 0.2.0) : le dossier n'est pas un pilier neuf mais
  la **première application** du noyau — c'est aussi son meilleur test.
  Proposition : C0–C1 comme validation de la 0.2.0, le reste après.
- **Sur-généricité de la chronologie** : neuf sources, autant de
  formats. Parade : un trait `TimelineSource` dans `pnex-core`, une
  implémentation par source, plafonds par source, tests de contrat.
- **Glissement vers la surveillance de personnes** : produit open source,
  usages imprévus. Parades : non-objectifs §4 (pas de collecte
  automatisée de personnes, pas d'identification biométrique), K11
  (rien sans revue humaine), K16, documentation d'usage responsable.
- **Fausse confiance** dans le statut `corroborated` : deux sources
  distinctes peuvent recopier la même origine. L'UI affiche les pièces,
  jamais un badge seul.
- **Confusion confinement / ouvert** : un journaliste qui crée un dossier
  « ouvert » dans une org partagée expose ses sources. Le modèle Enquête
  propose le confinement par défaut et l'UI affiche la visibilité en
  permanence.

## 11. Questions à trancher (appartiennent au produit)

1. Version cible : C0–C1 en validation de la 0.2.0 (exception au gel),
   le reste en 0.3 ? Ou tout après la 0.2.0 ?
2. Nom définitif : « Dossiers / Cases » (proposé) ou autre. **Collision** :
   « Dossier » est déjà le libellé fr-FR des `folder` (`onto-type-folder`) ;
   libellé provisoire du code : « Dossier d'enquête » / « Case ».
3. ACL par dossier (K9 V2) : à rouvrir avec D188, dans la 0.3 avec les
   ACL par objet, ou jamais (org dédiée suffit) ?
4. Grille de crédibilité A–F × 1–6 : dans le pack par défaut ou en
   option avancée ?
5. Temps flou (K5) : propriété du noyau (proposé, utile à Repère et aux
   packs Maintenance — « date de mise en service approximative ») ou
   limité au pack ?
6. `person` / `organization` : dans un pack de base « Acteurs » requis
   par Enquête et Couverture médiatique, ou déclarés par chacun avec
   fusion à l'installation ?
7. Co-présences GPS (K10) : en V1 ou plus tard (sensibles, coûteuses) ?
8. Faut-il un horodatage qualifié (RFC 3161) des gels pour une valeur
   probante, ou rester explicitement hors champ ?
9. Apparitions (K17) : seuil de confiance par défaut sous lequel un
   rattachement part en revue ; écriture automatique au-dessus du seuil
   par le flow déployé, ou tout en revue en V1 ?
10. Identification vocale (K19) : enrôlement consenti seulement,
    variante B (sources publiques, orgs de presse, dérogation
    journalistique), ou non-objectif maintenu ? Avis juridique RGPD art. 9
    + AI Act haut risque requis dans les deux premiers cas.

## 12. Avancement (amorce C0–C1, 2026-10-11, non validé produit)

Amorce faite à la demande (« commence à bosser »), réversible, avant
arbitrage des questions §11.

| Élément | Choix | État |
|---|---|---|
| Type `case` (K1) | type **système sans table native** : objets dans `objects`, écrits par l'API générique `/api/v1/ontology/objects` ; `ObjectTypeDef::native()` distingue les types D42 (table propre, créés par leur page) des types système « purs » ; propriétés intégrées `question`, `status` (requis, `open`/`concluded`/`archived`), `window_from`/`window_to`, `template` ; extension d'org ajoutée **après** les propriétés intégrées (jamais à leur place) ; sous-dossiers par containment | code |
| Lien `in_case` (K2) | lien système `*` → `case`, attributs `role` (requis, 6 valeurs), `pinned`, `note` ; retrait = fermeture (D179) | code |
| Chronologie (K6) | `POST /api/v1/cases/{id}/timeline` : membres (liens `in_case` ouverts) + ouvertures/fermetures des liens des membres dans la fenêtre (base) + changements d'objets (O2 `object_changes`, une requête `IN`) ; fenêtre = requête, sinon fenêtre du dossier, sinon 30 j ; plafond 2 000 par source (`truncated`) ; `changes_available` si O2 absent. **Sources restantes** : alertes D86, caméra, transcriptions, plages, séries, apparitions K17 | code partiel |
| UI | `/cases` (création en une ligne + Entrée, filtre de statut) ; `/cases/:id` : en-tête éditable sur place (enregistré à la sortie du champ, `expected_version`), membres par rôle avec ajout à la frappe (↑↓ Entrée, rôle en puces), composeur de lien daté entre membres (types de lien filtrés par les deux bouts), chronologie par jour, filtre par membre, préréglages 24 h / 7 j / 30 j / 1 an ; « Ajouter à un dossier » sur la page objet et les pages device, média, POI (via `ObjectPageLink`) | code |
| Assistant | fiche `assistant-kb/cases.md` ; schéma généré (D189) inclut `case` et `in_case` sans code ; outils dédiés K15 non faits | partiel |
| Libellé | « Dossier d'enquête » / « Case » (collision « Dossier » = `folder`, §11 q.2) | provisoire |
| Sources de chronologie (K6, suite) | plages à portée `object` d'un membre (D182) ; notifications des flows membres (O2 `notify_deliveries`, `flow_id`) ; dates floues des membres (`occurred`) ; anciens membres retirés dans la fenêtre gardés en couloir | code |
| Ouverture depuis un événement (K3, C2) | `POST /api/v1/cases` `{title, from: {kind: notification\|event, at, flow_id?, margin_minutes?}}` : fenêtre ±2 h (1 min–7 j), flow source en `subject`, ses voisins à l'instant `t` en `context` ; flow d'une autre org ignoré (R1) ; bouton « Ouvrir un dossier » dans le détail d'un événement (`/events`) et d'une notification (journal de canal) | code |
| Temps flou (K5) | `PropertyKind::FuzzyTime` dans le noyau (proposition q.5, provisoire) : `{from, to, precision}`, bornes UTC `Z`, `instant` ⇒ égales ; `fuzzy_bounds` dérive les bornes d'une date à sa précision (saisie en un champ) ; affichage à la précision (`2019-03`, `≈ a → b`) | code |
| Pack `investigation` (K4) | personne, organisation, événement (`occurred` flou), lieu (`place`, 1.1.0), modèles de pages et `corroboration_note` (1.2.0), pièce, affirmation, hypothèse, identifiant ; liens `involves`, `occurred_at`, `located_at` (1.1.0), `before`, `member_of`, `employs`, `owns`, `knows`, `uses_identifier`, `wraps`, `about`, `sourced_by` (`locator`), `supports`, `contradicts`. Pas de `requires` (aucun pack Couverture médiatique livré à ce jour) | code |
| Corroboration (K7) | `corroborated` refusé (409 `ontology-claim-unsourced`) sans ≥ 2 pièces `sourced_by` d'origines distinctes (URL source, sinon objet enveloppé, sinon la pièce) ; vérifié au changement de statut. Dérogation justifiée : voir plus bas | code partiel |
| Tableau des hypothèses (K8, C7) | `GET /api/v1/cases/{id}/board` : chaque `hypothesis` membre avec ses `claim` `supports` / `contradicts` (statut, nombre de sources ouvertes), puis les affirmations membres **sans source** et **contredites** par une autre ; aucun score ; outil assistant `case_board` (lecture) | code |
| Pages et conclusion (K8, P13, C7) | les packs portent des **modèles de pages** (`page_templates` : clé, nom, description, corps en Markdown réduit — titres, listes, paragraphes) ; le pack `investigation` en livre cinq : 5 pourquoi, Ishikawa (6M), compte rendu d'incident, note d'enquête, script / conducteur (en anglais comme les types du pack). `GET /api/v1/cases/templates` (packs installés par l'org) ; `POST /api/v1/cases/{id}/pages {title, template?}` crée la page **remplie côté serveur** (arbre `y-prosemirror` construit en yrs, validé par la liste blanche P15) et la rattache en `note`. Propriété intégrée `conclusion_page` (réf. `doc`) ; au passage du statut à `concluded` (service objets générique, donc aussi par l'API et l'assistant), version nommée « Conclusion » de la page — un échec est journalisé sans annuler le statut. Panneau « Pages » de l'espace de travail : création en une ligne + Entrée, la page s'ouvre aussitôt. Raccourcis : pas de modèle par type d'objet (`@this`, P13), modèles non traduits, l'export n'embarque pas encore le texte de la conclusion | code |
| Rétention et purge (K16, C7) | `GET /api/v1/cases/{id}/purge` : `person` et `identifier` membres du dossier (ouverts ou fermés), membres d'aucun autre dossier (même fermé), sans lien ouvert hors des membres du dossier (une personne et son numéro, tous deux membres, partent ensemble) ; `POST …/purge {ids}` sur un dossier **archivé** seulement (sinon 400 `status`), liste recalculée côté serveur, rôle d'écriture du type vérifié : **suppression réelle** (seule exception à D179) de l'objet, de tous ses liens, de ses plages `object`, labels et rangement, profil vocal en cascade ; le journal note `object.purge` sans recopier les données. Panneau « Données personnelles à purger » sous l'en-tête d'un dossier archivé, tout coché, confirmation. Raccourci : les copies déjà dans le journal O2 restent jusqu'à sa rétention | code |
| Dérogation K7 et rejeu des statuts (K7, K12, C7) | propriété `corroboration_note` de `claim` (pack 1.2.0) : un texte non vide autorise `corroborated` sous deux origines ; le tableau la montre (« dérogation justifiée », texte au survol). Export à l'instant du gel : propriétés des membres et statuts des hypothèses/affirmations **rejoués** depuis le journal (`before` du premier changement après l'instant, l'objet inchangé depuis garde son état) ; `states_replayed` = faux sans journal. Raccourci : 5 000 changements lus au plus après l'instant ; le tableau et le dossier à l'instant à l'écran gardent les statuts du jour | code |
| Gel et export (K12, K16, C7) | propriétés intégrées `frozen_at` (instant du gel, un seul — pas de versions nommées multiples) et `legal_basis` (base de traitement, texte libre) ; `POST /api/v1/cases/{id}/export` `{as_of?}` (défaut : gel, sinon maintenant) → un JSON : dossier, membres à l'instant avec leur objet, liens entre membres à l'instant, chronologie jusqu'à l'instant, tableau, manifeste des pièces (URL, SHA-256), nombre de `person` (avertissement UI). Lecture seule (tout membre de l'org). **Limites** : statuts des affirmations = ceux d'aujourd'hui (rejouer via O2 à faire), pas de YAML/CSV séparés, pas de vérification de divergence SHA-256, purge K16 à l'archivage non faite | code partiel |
| Vues synchronisées (K14, C4) | `GET /api/v1/cases/{id}/snapshot?as_of=` → membres présents à l'instant, liens valides entre eux, **lieux** : propriété `geo_point` du membre, sinon cible d'un lien de localisation ouvert à l'instant (`placed_at` D191, `occurred_at`, `located_at`) vers un POI (coordonnées `map_pins`) ou un objet à `geo_point`. Pack `investigation` 1.1.0 : type `place` (adresse, `location`) et lien temporel `located_at` (`*` → POI \| `place`, attribut `kind` vu/domicile/travail/immatriculé/entreposé/autre, plusieurs ouverts à la fois) — `placed_at` ne vaut que device → POI, un seul. UI : panneau « Le dossier à un instant » (curseur pas à pas sur les instants de la chronologie, graphe L4 + carte `pnexMap` côte à côte, clic nœud = filtre du couloir) ; clic sur l'heure d'une ligne de chronologie = curseur, événements postérieurs atténués. Outil assistant `case_snapshot`. **Limites** : `geo_point` à sa valeur du jour (rejeu O2 à faire), positions GPS live (D38) et traces non lues (imports C5), pas de filtre de carte par fenêtre (le curseur suffit en V1) | code partiel |
