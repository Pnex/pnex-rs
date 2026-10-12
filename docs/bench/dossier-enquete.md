# Benchmark — Dossier d'enquête (phase 0 de la refonte)

**Date :** 2026-10-12 · **Pour :** PRD `docs/architecture/cases-workspace.md`
(refonte de l'écran dossier, phase 0 « benchmark open source »).
**Fiches complètes** (une par projet, 9 sections, références
`chemin:ligne`) : `docs/bench/dossier-enquete/*.md`. Ce fichier en est la
synthèse et la seule partie à lire pour décider.

## 0. Méthode et limites

- Six dépôts clonés en `--depth 1` dans `~/Documents/shan-perso/bench/`,
  **hors du workspace PNEX** ; le schéma FollowTheMoney (dépendance
  d'OpenAleph) cloné à côté ; les sous-modules d'IRIS initialisés pour
  avoir du code à lire.
- Étude par **lecture du code source** (modèles, composants UI, API,
  algorithmes) et de la doc officielle ; issues GitHub/Codeberg citées
  comme preuves des pièges. Six références tirées au hasard ont été
  revérifiées dans les clones (toutes exactes).
- **Rien n'a été lancé en local** : OpenCTI exige Elasticsearch, Redis,
  RabbitMQ et un stockage S3 livré avec MinIO (banni chez nous) ;
  Timesketch exige OpenSearch ; IRIS une stack Docker complète. Les
  patterns UX sont donc décrits d'après le code et les captures de la doc
  (citées dans chaque fiche, §4). Si un point d'UX reste douteux, la démo
  publique de kepler.gl et les captures suffisent à trancher ; lancer
  Timesketch ou IRIS ne changerait aucune décision ci-dessous.
- Aucune capture copiée dans le dépôt (poids, droits) : chaque fiche
  donne le chemin dans le clone ou l'URL publique.

| Projet | Commit étudié | Licence confirmée | Ce que PNEX (MIT) peut en faire |
|---|---|---|---|
| OpenAleph (+ FollowTheMoney 4.11, react-ftm) | `82fed2e` (2026-10-09) | MIT partout | Reprise avec attribution |
| OpenCTI | `5f0be2a` (2026-10-11) | Apache-2.0 par défaut, **en-tête par fichier** : `enterprise-edition/`, playbooks, IA, audit = licence EE | CE : reprise avec attribution ; EE : rien. Investigations, cases et graphe sont CE |
| Timesketch | `f0b3602` (2026-10-09) | Apache-2.0 (en-têtes vérifiés) | Reprise avec attribution (Python/Vue : surtout de l'inspiration) |
| DFIR-IRIS | `37c5521` (2026-09-23) | LGPL-3.0 ; **nouveau front `iris-frontend` en AGPL-3.0** ; fork IRIS-NG (`zach115th/iris-ng`) LGPL-3.0 | **Inspiration seule, aucune ligne reprise** |
| kepler.gl | `c9f6ecb` (2026-10-12) | MIT (deck.gl MIT aussi) | Reprise avec attribution |
| MovingPandas | `b321da5` (2026-09-30) | BSD-3-Clause | Portage Rust avec notice et 3 clauses |

## 1. Fiches courtes

### 1.1 OpenAleph — `dossier-enquete/openaleph.md`

Investigations (collections `casefile`) contenant des entités FtM, des
frises, diagrammes réseau et listes (EntitySet à layout JSONB).

- **Reprendre** : dates **à précision explicite** (`2019-03` ≠
  `2019-03-01`) avec bornes basse/haute dérivées ; `temporalExtent`
  déclaré par type (quelles propriétés datent l'objet) et **rôle porté
  par l'attache**, pas par un sous-type ; expansion de nœud **en deux
  temps** (compteurs par type de lien, puis chargement, nouveaux nœuds
  sélectionnés, annulable).
- **Éviter** : frise plafonnée au jour, heures ignorées (#3009) ; layout
  entier réécrit à chaque geste, dernier qui écrit gagne (#2453) ;
  règles « datable » incohérentes, éléments sans date qui disparaissent
  du Gantt, arêtes fantômes (#2574).
- Graphe : SVG maison + d3-force + dagre, pas Cytoscape.

### 1.2 OpenCTI — `dossier-enquete/opencti.md`

Investigations (graphe de pivot, liste plate d'ids) et Cases (Incident
Response, RFI, RFT) contenant des refs ; graphe react-force-graph 2D/3D.

- **Reprendre** : expansion **à facettes avec compte restant**, calculé
  côté serveur ; **journal d'opérations** du graphe avec rollback
  jusqu'à la dernière expansion ; réglette de période avec histogramme
  qui **estompe** sans retirer.
- **Éviter** : force-directed sans garde-fous de volume (#1437, #9293,
  #10586 ; bug de précédence `links.length ?? 0 < 200`,
  `Graph.tsx:107`) ; **un seul instant par objet**, `created_at` comme
  date par défaut, dates sentinelles 1970/5138 (#6079, #3903) ; état de
  travail éclaté entre serveur, sessionStorage et URL, pont
  investigation → case qui perd positions et notes (#15058).

### 1.3 Timesketch — `dossier-enquete/timesketch.md`

Sketch = timelines d'événements bruts (OpenSearch) annotés : étoiles,
tags, commentaires, faits liés à des questions d'enquête.

- **Reprendre** : **ancre paresseuse** (une ligne SQL n'existe qu'à la
  première annotation d'un événement brut, la donnée brute n'est jamais
  copiée) ; fait = lien typé avec **cycle de revue** (proposé → vérifié
  | rejeté) et **signature humaine** ; histogramme cliquable, plages
  désactivables, **contexte ±Δ** réversible.
- **Éviter** : annotations en **bascule** (un double envoi retire
  l'étoile, #3434) ; annotations sans portée ni auteur écrites dans la
  donnée brute, analyseurs qui écrivent d'office ; récit en blob JSON
  réécrit en entier, verrou côté client ; lignes CSV à date illisible
  **jetées en silence**.

### 1.4 DFIR-IRIS — `dossier-enquete/iris.md`

Case + timeline d'événements (instants) liés à des assets/IOC ;
graphe vis-network dérivé des événements.

- **Reprendre** (en idée) : **timeline brute séparée de la timeline de
  travail** avec promotion explicite, idempotente, par lot (IRIS-NG
  `case_working_event`) ; événement = pivot auquel on attache, plusieurs
  vues sans duplication ; graphe **dérivé** des attaches, disposition à
  graine fixe, figé après stabilisation, sélection sans reconstruction.
- **Éviter** : fuseaux en offset texte, dates naïves affichées en local
  (#722, #730, #1054) ; export CSV qui ne se réimporte pas (#639),
  import non atomique ; tout charger puis filtrer en mémoire, graphe en
  clique O(k²).
- La frise visuelle vis-timeline a **disparu** de la v3 (liste
  verticale) : signe que leur frise n'était pas utilisable, pas que la
  frise est inutile.

### 1.5 kepler.gl — `dossier-enquete/kepler.md`

Trip layer : LineString `[lon, lat, alt, ts]`, traînée animée par un
seul paramètre `currentTime` (shader deck.gl), filtre temporel à fenêtre.

- **Reprendre** : temps canonique en **ms entières**, rendu **relatif au
  début du domaine** (précision float32) ; **deux horloges** (instant
  pour la carte, période pour le filtre) unies par un mode de synchro ;
  position au curseur par **dichotomie + interpolation**, rien de
  recalculé par frame.
- **Éviter** : vitesse « 1x » = domaine en 600 frames, dépendante du
  framerate (#740) ; détection de timestamps fragile sur le premier
  trajet seulement (#2526, #2287, #1117, #2813) ; pas de chemin binaire,
  traînée plafonnée à 1 000 s (#2939, #2089 ; deck #8927).
- **Avis** : rester sur **MapLibre seul** (traînée découpée côté wasm
  sur `[t − traînée, t]`, quelques tranches d'opacité) ; deck.gl
  `MapboxOverlay` ≈ +250 ko gzip, justifié seulement au-delà de ~100 000
  sommets visibles.

### 1.6 MovingPandas — `dossier-enquete/movingpandas.md`

Bibliothèque de trajectoires (pas d'UI) : arrêts, découpages,
généralisation, nettoyage, intersection avec polygones, interpolation.

- **Reprendre** (portage Rust) : détecteur d'arrêts (fenêtre glissante,
  diamètre < d strict, durée ≥ t) ; découpage au **trou de signal**
  (`> gap` strict) ; généralisation **TDTR** (temps-distance, tolérance
  par point précalculée → adaptation au zoom) ; clip ligne/polygone avec
  **entrée/sortie interpolées** (geofence).
- **Éviter** : degrés pris pour des mètres (DP, TDTR, rectangle minimal
  en coordonnées brutes) → projeter en ENU local ; fuseaux écrasés par
  `tz_localize(None)` (#303, #421) ; O(m² log m) pendant un arrêt long.
- **Co-présence absente** : algorithme exact proposé (balayage par paires
  de segments synchronisés, distance relative quadratique en t, O(n+m)
  par paire, « inconnu » au-delà d'un trou de signal, hystérésis,
  préfiltre rstar pour N acteurs). Crates : `geo`, `geographiclib-rs`,
  `rstar` (MIT/Apache) — aucune n'est encore dans le `Cargo.lock`.

## 2. Ce qui converge (les décisions que le benchmark impose)

| # | Enseignement | Vu chez | Conséquence pour PNEX |
|---|---|---|---|
| B1 | **Le brut et le retenu sont deux couches.** Le travail se fait en promouvant, jamais en écrivant dans le brut ni en ajoutant d'office | IRIS-NG, Timesketch (questions), OpenCTI (investigation → case) | Une seule file de **propositions** (détecteurs, signaux bruts, imports) → « Retenir » crée l'événement et ses attaches en une transaction ; rejet conservé pour ne pas re-proposer |
| B2 | **Le temps de l'événement est un temps du monde**, avec durée et incertitude ; jamais `created_at` | OpenCTI (piège), OpenAleph (précision), IRIS (fuseaux) | `t_start`/`t_end` + précision ; UTC stocké, fuseau d'affichage explicite et étiqueté ; échelle continue de l'année à la seconde |
| B3 | **Le rôle vit sur l'attache**, pas dans le type | OpenAleph (Interest.role), IRIS (manque comblé), Timesketch | Lien événement → objet porteur d'un `role` fermé |
| B4 | **Les objets sans date restent visibles** | OpenAleph (piège Gantt), OpenCTI (sentinelles) | Bac « Non daté » permanent ; inconnu = NULL, jamais une date magique |
| B5 | **Graphe dérivé, borné, stable** : dérivé des attaches, layout déterministe, plafond de nœuds, labels d'arêtes au survol, expansion à compteurs et annulable | OpenCTI, IRIS, OpenAleph | Graphe biparti objets–événements calculé côté Rust, rendu SVG natif |
| B6 | **Pas de blob réécrit en entier** : chaque geste est une écriture unitaire versionnée | OpenAleph, Timesketch, OpenCTI | Un événement = un objet versionné (`expected_version` → 409), une attache = un lien ; gel = instantané matérialisé + SHA-256, vérifié côté serveur |
| B7 | **Intentions explicites, pas de bascule** | Timesketch #3434 | API d'état cible (retenir/rejeter, attacher/détacher), idempotente |
| B8 | **Normaliser le temps à la frontière, en Rust** | kepler, IRIS, Timesketch | Import et lecture O2 → `i64` ms UTC triés, doublons fusionnés, rejets **listés** à l'utilisateur, fuseau demandé si la source est naïve |
| B9 | **Rien de recalculé par frame** : la trace est chargée une fois par fenêtre, la lecture ne fait que déplacer un instant ; vitesse = temps réel × facteur | kepler | Horloge `performance.now()` × facteur dans un signal local ; dichotomie + interpolation dans `pnex-core` (wasm et serveur) ; pas d'interpolation à travers un trou de signal |
| B10 | **Un seul objet dossier** (pas de couple « investigation / case » à synchroniser) | OpenCTI (piège), Timesketch | L'espace de travail **est** le dossier |
| B11 | **Export qui se réimporte** | IRIS #639 | Schéma d'échange versionné, mêmes en-têtes JSON/CSV, test d'aller-retour |

## 3. Réponses aux questions ouvertes du PRD

**Q1 — Traces : OpenObserve seul, ou adaptateur CSV/GPX ?** Il faut
l'adaptateur. Le persona « données seules » n'a aucun device, et les cinq
projets qui touchent au sujet convergent : le fichier importé est gardé
comme **pièce** (média, SHA-256), ses points sont **normalisés en Rust et
ingérés une fois dans O2** sous une origine `import:<média>` (supprimable
et rejouable, à la manière du mapping d'Aleph), puis lus **par la même
interface de trace** que le GPS des devices. Rien n'est copié dans
Postgres. Un seul chemin de lecture, au format kepler en sortie.

**Q2 — Graphe Rust natif dans Dioxus, ou Cytoscape.js ?** **Natif.**
OpenAleph (SVG + d3-force), IRIS-NG (passé à D3) et OpenCTI montrent que
l'usage réel est modeste (layout force, sélection, estompage, expansion)
et que les problèmes viennent du volume, pas du moteur. À l'échelle d'un
dossier (dizaines à quelques centaines de nœuds), un layout de forces
**déterministe** calculé dans `pnex-core` (graine = id du dossier, N
itérations puis arrêt) et un rendu SVG Dioxus suffisent, sans pont JS ni
dépendance. PNEX rend déjà son graphe d'ontologie en SVG natif. Seul
Timesketch plaide pour Cytoscape, parce que le graphe y est secondaire ;
ce n'est pas notre cas (synchro avec la frise, estompage au curseur). On
garde Cytoscape en réserve si des layouts hiérarchiques (dagre, elk)
deviennent nécessaires.

**Q3 — Nom.** Les projets séparent l'objet et la démarche :
« Investigation » (Aleph), « Case » (OpenCTI, IRIS), « Sketch »
(Timesketch). Trois fiches sur quatre recommandent **« Enquête »** comme
libellé court en français (évite la collision avec « Dossier » =
`folder`, déjà prise dans l'UI) ; deux gardent « Dossier d'enquête » pour
l'objet exporté ou gelé. « Projet » est écarté par toutes (trop générique,
collision avec les projets firmware D87). Proposition au PRD : **libellé
de navigation « Enquêtes »**, titre long « dossier d'enquête » là où le
contexte l'exige, type système `case` inchangé. À valider par Shan.

## 4. Ce que PNEX n'a pas encore (inventaire du 2026-10-12)

Ce que la refonte suppose et qui n'existe pas dans le code :

| Besoin | État actuel | Écart |
|---|---|---|
| Trace `[lon, lat, alt, ts]` sur une fenêtre | `device_positions` = dernière position seulement ; historique = deux séries O2 séparées `latitude` / `longitude` | Pas d'API de trace ; appariement lat/lon à refaire à la lecture |
| Geofences | Aucun polygone, aucun point-dans-polygone ; PostGIS non installé (`geo-layers.md` proposé) | À créer |
| Présence BLE (badges) | Rien | Hors V1, sauf comme source de positions déclarée |
| Carte : lignes, traînées, couches | `map.js` (MapLibre 6) ne pose que des marqueurs HTML | API de couches GeoJSON à ajouter |
| Graphe : layout de forces, `as_of`, expansion | `graph_view.rs` radial à anneaux fixes ; `/graph` sans `as_of` | Layout et rendu à refaire |
| Événement daté au noyau | `event` n'existe que dans le pack `investigation` (date floue, lieu) | À promouvoir dans le noyau |
| Attache avec rôle d'enquête | `involves` (pack) : rôle en texte libre ; `in_case` : 6 rôles d'appartenance | Rôle fermé sur l'attache |
| Propositions, instantanés | Un seul `frozen_at`, rejeu via le journal O2 (rétention bornée) | Tables dédiées |
| Frise horizontale à lignes | Chronologie actuelle = liste verticale par jour | À créer |
| Glisser-déposer, raccourcis globaux | Gestes pointeur dans les éditeurs ; pas de registre de raccourcis | Patrons existants à réutiliser |
| Séries à bornes absolues | `/series/{prop}?window=` = préréglages relatifs seulement | Bornes absolues à ajouter |

## 5. Écarts entre le prototype du 12/10 et PNEX

Le prototype navigable reste la spec d'**usage**. Trois écarts à corriger
avant d'en faire la maquette de référence (phase 1) :

1. **Identité visuelle** : il utilise Fira Sans, un accent orange
   `#ff5b3a` et des libellés en capitales ; l'identité PNEX
   (`design-system.md`) est IBM Plex, accent `signal` `#0e7c9c`, `mark`
   `#e5482d`, sans capitales (garde `design_guard`). Les maquettes sont
   refaites aux jetons PNEX.
2. **Graphe Cytoscape** → graphe natif (Q2) ; la maquette ne doit
   montrer que ce que le rendu natif fera (pas d'animation de layout).
3. **Détections ajoutées d'un clic directement dans le dossier** → elles
   passent par la file de propositions (B1), avec la mention de ce qui a
   été calculé et avec quels seuils.
