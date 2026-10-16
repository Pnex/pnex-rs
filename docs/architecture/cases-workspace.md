# PRD — Espace de travail du dossier d'enquête (refonte)

| | |
|---|---|
| Statut | **v0.2 — 2026-10-12**, après benchmark (phase 0 faite). v0.1 = brouillon de Shan du même jour |
| Cible | PNEX 0.2.0 (noyau ontologique) |
| Stack | Loco (API) · Dioxus CSR (web) · PostgreSQL · OpenObserve · MapLibre |
| Référence UX | Prototype navigable « PNEX Dossier d'enquête » (artifact du 12/10/2026, données d'exemple) — spec d'**usage**, pas d'identité visuelle (§11) |
| Benchmark | `docs/bench/dossier-enquete.md` (synthèse) + `docs/bench/dossier-enquete/*.md` (fiches) |
| Relation à `cases.md` | `cases.md` (K1–K19) reste la référence du **modèle d'enquête** (pack, affirmations, apparitions, voix). Ce PRD remplace son **écran** (K6 rendu, K14) et amende K1, K2, K6 (§9). Décisions numérotées **K20…K47** à la suite |
| Suite | `cases-money.md` (K48–K64) : navigation (filtres, chemins, regroupement, preuves en couche) et flux financiers (montant, versement, détention, vue Argent) |

## 1. Problème

L'écran actuel est un formulaire de métadonnées (question, fenêtre, base
RGPD, gel), suivi d'une liste verticale par jour. Ce n'est pas un espace
de travail.

- L'objet central d'une enquête, l'**événement daté**, n'existe pas dans
  le noyau. Il n'existe que dans le pack `investigation`, qu'un dossier
  RCA n'a aucune raison d'installer.
- Les membres n'ont pas de date : le curseur temporel n'a rien à
  parcourir. Le curseur actuel saute d'instant de chronologie en instant
  de chronologie.
- On choisit un rôle d'appartenance avant de choisir un objet, et ce rôle
  ne dit pas *dans quel fait* l'objet intervient.
- Pas de carte de trajets (PNEX ne garde que la dernière position d'un
  device), pas de graphe utile (anneaux fixes), pas d'état vide qui
  guide. Erreur visible « Impossible de charger la chronologie ».
- Résultat : on ne comprend pas à quoi sert l'outil.

## 2. Objectif

Un dossier où l'on reconstitue **qui, où, quand, et avec quelles
preuves**, en manipulant directement le temps, la carte et les liens,
sans formulaire. Même outil pour une RCA industrielle, un enquêteur data,
un journaliste ou un vidéaste.

**Succès mesurable**

- Un nouvel utilisateur pose son premier événement et y attache une
  pièce en **moins de 60 s sans aide** (test d'usage phase 1, 3
  personnes, chronométré).
- **Lecture fluide** : rejouer un dossier de 3 acteurs sur 2 h tient
  **≥ 30 images/s** sur un laptop ordinaire, avec la stack serveur sur
  Raspberry Pi 5. Côté serveur, le budget porte sur le **chargement** :
  traces d'une fenêtre servies en < 300 ms au p95 sur Pi 5 (3 acteurs,
  2 h, 1 point/10 s), < 1,5 s pour 20 acteurs sur 24 h à 1 Hz grâce à la
  généralisation (K27). La lecture elle-même ne fait **aucune requête**.
- **Zéro écran vide sans action proposée** (garde e2e : chaque état vide
  porte un bouton ou un geste).

## 3. Hors périmètre (v1)

- Chaîne de possession à valeur légale (le gel reste un instantané
  exportable, sans horodatage qualifié).
- Collaboration temps réel multi-curseurs (après la v1 ; la concurrence
  optimiste suffit, K35).
- Nouvelles sources de capteurs (BLE, UWB, RFID) : le dossier consomme
  devices, médias, POI et mesures déjà dans PNEX, plus les **imports de
  traces** (K26), nécessaires au persona « données seules ».
- Polygones dessinés à la main sur la carte (v1 : cercles dessinés,
  polygones importés, K29).
- Fusion de dossiers, dossiers entre orgs, ACL par dossier (`cases.md`
  K9 V2).

## 4. Démarche (ordre obligatoire)

**Benchmark → UI design → modèle → code.** Aucune ligne de Dioxus avant
la validation de la phase 1.

| Phase | Livrable | Porte de sortie | État |
|---|---|---|---|
| 0 — Benchmark | `docs/bench/dossier-enquete.md` + 6 fiches | Lu par Shan | **fait 2026-10-12** |
| 1 — UI design | Maquettes haute fidélité aux jetons PNEX (§11), test d'usage, liste des frictions des écrans voisins | Maquettes validées par Shan | **en cours** : maquettes v1 publiées le 2026-10-12 (artifact « Enquête PNEX · maquettes », 10 états, clair/sombre, téléphone) ; restent test d'usage et frictions des écrans voisins |
| 2 — Modèle | Migration (§7), contrats `pnex-core`, tests de modèle | Revue du schéma par Shan, `/security-review` | après phase 1 |
| 3 — Code | Lots L0–L6 (§12) | Sortie vérifiable par lot, e2e | après phase 2 |

## 5. Ce que dit le benchmark (rappel)

Onze enseignements convergents (synthèse §2) fondent les décisions :

- B1 : le brut et le retenu sont deux couches, on travaille en
  promouvant ;
- B2 : le temps de l'événement est un temps du monde ;
- B3 : le rôle est porté par l'attache ;
- B4 : les objets sans date restent visibles ;
- B5 : le graphe est dérivé, borné et stable ;
- B6 : pas de blob réécrit en entier ;
- B7 : des intentions explicites, pas de bascule ;
- B8 : le temps est normalisé à la frontière ;
- B9 : rien n'est recalculé par frame ;
- B10 : un seul objet dossier ;
- B11 : un export qui se réimporte.

Réponses aux questions ouvertes de la v0.1 : §15.

## 6. Décisions

### K20 — L'événement est un type du noyau

- Type système `event` (identité universelle, objets temporels comme
  les autres), **sans table native**, à l'école de `case` (`cases.md`
  §12) : propriétés intégrées `occurred` (`fuzzy_time` : `from`, `to`,
  `precision`), `note` (texte d'analyse, 4 000 car.), `origin` (`manual`
  | `detection` | `signal` | `import`), `location` (`geo_point`,
  optionnelle : lieu fixé à la main quand aucune pièce ne le donne).
- **Instant** = `precision` ≠ `range` et `from == to` à la précision
  près ; **période** = `precision = range`. Les deux formes de la v0.1
  (`t_start`, `t_end` nullable) sont donc portées par la valeur floue
  existante, sans nouveau type de propriété. Une période ouverte
  (« depuis 14:02, fin inconnue ») = `to` absent, rendue en barre qui
  s'estompe vers la droite.
- Le pack `investigation` passe en **1.3.0** et ne déclare plus `event`
  ni `involves` : il les **utilise**. Ses objets `event` existants sont
  repris tels quels (mêmes clés de propriétés ; `summary` devient
  `note`).
- Pourquoi un objet et pas une ligne `case_event` : un même événement
  (« alarme HP ligne 3 à 14:32 ») sert deux dossiers (RCA et dossier
  qualité) ; il gagne recherche, page objet, provenance, journal,
  assistant et export sans code. Le coût d'écriture d'un objet (schéma,
  une ligne, un ingest O2) est compatible avec un geste humain.

### K21 — L'attache : lien `involves` du noyau, rôle fermé

- Lien système `involves` : `event` → `*`, attributs `role` (requis,
  enum `evidence` | `actor` | `context` | `hypothesis` | `source`,
  libellés « Preuve, Acteur, Contexte, Hypothèse, Source ») et `detail`
  (texte libre, 255 car. : « témoin », « opérateur de quart »). Le texte
  libre de l'ancien `involves.role` du pack migre dans `detail`.
- **Rôle proposé à l'attache selon le type**, modifiable aussitôt :
  personne, organisation, véhicule → Acteur ; POI, lieu, zone →
  Contexte ; note → Hypothèse ; média, document, mesure, device →
  Preuve.
- Détacher **ferme** le lien (D179, comme `in_case`) : on sait ce
  qu'un événement portait au moment d'un gel. La fermeture se fait au
  temps système, pas au temps du monde (§16, Q-T1).
- Un même objet peut être attaché à plusieurs événements, avec un rôle
  différent dans chacun.

### K22 — Les membres se déduisent ; le bac « Non daté »

- **Membres du dossier** = objets attachés (lien `involves` ouvert) à au
  moins un événement du dossier ∪ objets du bac. On ne saisit plus de
  membres.
- **Événements du dossier** = objets `event` liés `in_case` au dossier
  (rôle `subject`). Un événement retiré du dossier ferme ce lien.
- **Bac « Non daté »** = objets liés `in_case` qui ne sont attachés à
  aucun événement du dossier. Attacher un objet du bac à un événement le
  sort du bac (le lien `in_case` est fermé dans la même transaction).
  Le bac n'est jamais masqué (B4).
- Les rôles d'appartenance actuels d'`in_case` (`actor`, `evidence`…)
  restent valides pour les données existantes et l'assistant, mais
  l'écran ne les montre plus : le rôle qui compte est celui de
  l'attache (B3).

### K23 — La frise d'abord, toujours visible en haut

- Règle temporelle **continue**, de l'année à la seconde (molette +
  Ctrl, boutons −/+, « Ajuster »), heures toujours lisibles (piège
  Aleph) ; graduation adaptative (pas 1 s … 1 an).
- Lignes, de haut en bas :
  1. **Événements** (ligne de travail) : clic = instant, glisser =
     période, glisser un événement = le déplacer (enregistré au
     relâcher, `expected_version`) ; empilement automatique des
     étiquettes ;
  2. **Signaux** (ligne brute, repliable) : alertes, notifications,
     changements d'objets, plages, détections proposées, calculés
     (K30) ; chaque signal se **retient** d'un geste (B1) ;
  3. **une ligne par acteur suivi** (membre qui a une trace, K25) :
     **zones de présence** nommées (POI, zone ou « hors zones »),
     **trous de signal** hachurés, repères des événements où il est
     attaché ;
  4. **une ligne par mesure** (membre à propriété `series`) :
     sparkline, seuils, valeur au curseur.
- **Curseur** unique (tête rouge) ; la partie postérieure au curseur
  est atténuée dans la frise, la carte et le graphe.
- Densité : sous la règle, une bande d'histogramme des signaux et
  événements (repère Timesketch/OpenCTI) pour voir où il se passe
  quelque chose avant de zoomer.
- **Fuseau** : stockage UTC ; la règle affiche l'heure dans un fuseau
  choisi et **toujours étiqueté** (« UTC », « Europe/Paris », « heure du
  navigateur »), réglage du dossier (piège IRIS).

### K24 — Synchronisation totale par un seul état de temps

- Un seul état partagé `TimeCursor { t, window, playing, speed }` :
  `t` pilote la carte (positions, traînées), le graphe (estompage) et
  l'inspecteur ; `window` (la portion visible de la frise) borne les
  chargements. Le zoom de la règle reste local au composant (kepler).
- Sélectionner un événement **déplace le curseur** à son début,
  **centre la carte** sur son lieu (lieu de l'événement, sinon POI
  attaché, sinon position interpolée du premier acteur attaché à cet
  instant) et surligne ses attaches dans le graphe.
- Lecture ×1/×3/×8 = **temps réel multiplié** (`t += Δhorloge ×
  facteur`, horloge `performance.now()`), indépendant du framerate
  (piège kepler) ; mode « ajusté » : toute la fenêtre en 60 s, pour les
  périodes longues.

### K25 — Acteurs suivis et sources de position

- Un objet a une **trace** si l'une de ces sources lui donne des
  positions sur la fenêtre :
  1. c'est un device qui publie la convention D38 ;
  2. il est lié par le nouveau lien système **`tracks`** (device → `*`,
     temporel) : « le traceur T donne la position du camion AB-123 du
     3 au 20 mars ». Comme `measures` (D181), la trace de l'objet est
     **recomposée** à partir des liaisons successives, chacune bornée à
     sa validité ;
  3. une **trace importée** lui a été rattachée (K26).
- Suivre un acteur (clic sur sa ligne) : frise, carte et graphe ne
  montrent que sa chronologie ; bouton « Tout afficher » pour revenir.

### K26 — Traces : flux et imports par la même lecture

- **Nouveau stream O2 `positions`** (logs, par org) : une ligne par
  position complète `{_timestamp, object_id, device_id?, lon, lat, alt?,
  accuracy_m?, speed_mps?, heading_deg?, origin}`. `origin` vaut
  `telemetry`, `import:<media_id>` ou `manual`.
- **Côté device** : `GpsTapSink` apparie déjà latitude et longitude
  pour la dernière position (D38) ; il écrit désormais **chaque** paire
  complète dans ce stream, par lots, à l'école du batcher O2 (on lâche
  sous pression, on ne bloque jamais le flux). L'historique antérieur à
  ce lot n'est pas rejoué : on n'apparie pas `latitude` et `longitude`
  après coup.
- **Import** (GPX, GeoJSON avec temps, CSV `lat, lon, ts[, alt]`) :
  - le fichier entre dans la médiathèque (pièce, SHA-256) ;
  - un **aperçu** montre le mapping des colonnes, l'unité de temps
    (déduite par magnitude sur **toute** la colonne, piège kepler) et
    le **fuseau**, obligatoire si les dates sont naïves ;
  - l'utilisateur choisit l'objet suivi ;
  - les points sont normalisés en Rust (tri stable, doublons fusionnés,
    points aberrants signalés) ;
  - un **rapport de rejets** liste chaque ligne écartée avec sa raison
    (piège Timesketch) ;
  - les points sont ingérés **une fois** dans `positions` sous
    `import:<media_id>`.
  - Supprimer l'import = `delete_stream_range` des points de cette
    origine.
- **Lecture** : `POST /api/v1/cases/{id}/tracks` (K27). Les points ne
  sont **jamais** copiés en base Postgres ; ils ne le sont dans un gel
  qu'en version généralisée (K33).

### K27 — Servir une fenêtre de traces, généralisée selon le zoom

- Entrée : `{from, to, objects?, resolution_ms}`. `resolution_ms` = durée
  représentée par un pixel de la frise.
- Sortie, par objet : `t0` (ms UTC) et des **segments** de points
  `[lon, lat, alt|null, dt_ms]` (temps relatif à `t0`, précision
  float32 côté carte, pattern kepler) ; une coupure de segment = **trou
  de signal** (écart > `signal_gap_s` du dossier, défaut 300 s,
  découpage MovingPandas `> gap` strict). L'appel renvoie aussi
  `truncated` et le nombre de points bruts.
- Généralisation **TDTR** (temps-distance, MovingPandas) dont la
  tolérance dérive de `resolution_ms` : on ne renvoie pas plus de
  détail que ce que la frise et la carte peuvent montrer. Plafond de
  20 000 points par réponse.
- Le client garde les fenêtres chargées ; la lecture et le glisser du
  curseur dans une fenêtre chargée ne font **aucune requête** (B9) ;
  zoomer arrière recharge plus grossier, zoomer avant recharge plus fin
  sur la portion visible seulement.
- Distances en mètres sur une **projection locale ENU** (jamais des
  degrés pris pour des mètres, piège MovingPandas), géodésique pour les
  distances affichées.

### K28 — Carte : trajets jusqu'au curseur, MapLibre seul

- `map.js` gagne une API de couches. Elle reçoit les traces **une fois**
  par fenêtre (`setTracks`). Elle reçoit aussi les zones et les épingles
  d'événement. À chaque image, elle ne reçoit que **`setTime(t)`**
  (quelques octets).
- Le JS calcule par dichotomie + interpolation la tête de chaque acteur
  et la portion passée de chaque trajet. Il met à jour une source GeoJSON
  par acteur, en **quelques tranches d'opacité** selon l'âge (traînée).
  Le trajet complet reste en pointillé discret.
- Pas de deck.gl : pas de poids JS en plus (≈ 250 ko gzip), pas de
  second moteur. On le réexamine au-delà de ~100 000 sommets visibles.
- **Pas d'interpolation à travers un trou de signal** : la tête
  disparaît, la ligne d'acteur montre le trou.
- Calques : fond (fournisseur géo de l'org, sinon fond uni), zones,
  POI, trajets, têtes d'acteurs (initiales, couleur de ligne), arrêts
  détectés, cercle de co-présence en cours, épingle de l'événement
  sélectionné, alarme sur un device en défaut à `t`.
- La logique d'échantillonnage existe donc deux fois, en Rust
  (`pnex-core`, pour les détecteurs et l'inspecteur) et en JS (pour
  l'image). Un **jeu de tests commun** (fixtures JSON) les tient
  identiques : interpolation, trou, doublon, bornes.

### K29 — Zones (geofences)

- Nouveau type de propriété **`geo_area`** : un cercle `{center,
  radius_m}` ou un polygone GeoJSON (un anneau extérieur, ≤ 1 000
  sommets, fermé, non auto-sécant). Il est validé dans `pnex-core`,
  natif et wasm.
- Déclaré sur le type système `map_pin` (propriété d'extension
  « zone ») et sur `place`. Une zone est donc un POI ou un lieu qui a
  une emprise.
- En v1, on dessine un **cercle** sur la carte (clic puis glisser le
  rayon) ; le **polygone** s'importe (GeoJSON) ou se colle. Le
  point-dans-zone tourne en Rust (crate `geo`, MIT/Apache).
- Quand PostGIS arrivera (`geo-layers.md`), une couche pourra servir de
  source de zones derrière la même interface.

### K30 — Signaux et détections : proposés, jamais ajoutés d'office

- **Signaux bruts**, calculés à la volée, sans copie et sans ligne en
  base tant qu'on ne les retient pas (ancre paresseuse, Timesketch).
  Le service de chronologie actuel devient leur source : alertes et
  notifications (D86), changements d'objets, ouvertures et fermetures
  de liens, plages, dates floues.
- **Détections**, calculées par des fonctions Rust pures de
  `pnex-core` sur les traces de la fenêtre :

  | Détecteur | Paramètres (défauts, réglables par dossier) | Algorithme |
  |---|---|---|
  | Co-présence | distance 40 m, durée ≥ 2 min, sortie à 1,2 × la distance | Balayage exact par paires de segments synchronisés (distance relative quadratique en t), O(n+m) par paire, préfiltre `rstar` pour N acteurs ; **inconnu** pendant un trou de signal |
  | Entrée / sortie de zone | zones des POI et lieux membres ou proches | Clip trace × zone, instants d'entrée/sortie interpolés, plages contiguës fusionnées sous 10 ms (MovingPandas) |
  | Arrêt | diamètre 25 m, durée ≥ 15 min | Détecteur d'arrêts MovingPandas (fenêtre glissante, diamètre strict, durée large), point = médiane |
  | Fin de signal | trou > 300 s ou trace qui s'arrête avant la fin de la fenêtre | Découpage au trou de signal |

- Les détections sont écrites dans **`case_proposals`** (§7) avec une
  **empreinte** (détecteur, objets triés, début arrondi à la
  résolution). Recalculer ne fait qu'insérer ce qui est nouveau : un
  rejet n'est jamais reproposé.
- « **Retenir** » crée, en une transaction : l'événement (`origin =
  detection` ou `signal`), ses attaches (rôles proposés), une note « à
  vérifier » qui cite le détecteur et ses seuils, et
  `source_ref = detector:<clé>@<version>` (D184). C'est **idempotent** :
  retenir deux fois rend le même événement (verrou de ligne).
  « **Rejeter** » est conservé. Les deux sont des intentions explicites,
  pas des bascules (B7).
- Calcul déclenché à l'ouverture du dossier et sur « Recalculer », borné
  à la fenêtre du dossier et en temps (budget 2 s sur Pi 5 pour 10
  acteurs × 24 h à 1 Hz). C'est un calcul **serveur** : il ne dépend
  pas d'un onglet ouvert.
- L'assistant peut lire les propositions ; il ne peut ni retenir ni
  rejeter (D123, D189).

### K31 — Le graphe : dérivé, natif, stable, borné

- Graphe **biparti** objets ↔ événements : un événement est un nœud
  (losange), une attache est une arête étiquetée par son rôle. Les
  relations d'ontologie entre membres (liens valides à `t`) sont en
  pointillé. Pas de clique entre objets (piège IRIS).
- **Rendu SVG natif Dioxus** ; layout de forces **déterministe** calculé
  dans `pnex-core` (graine = id du dossier, N itérations puis arrêt).
  Les positions existantes sont conservées quand un nœud s'ajoute
  (layout incrémental) ; un nœud déplacé à la main est épinglé pour la
  session. Remplace la vue radiale actuelle pour le dossier.
- **Estompage** au curseur : un événement postérieur à `t` et ses arêtes
  sont atténués, un objet dont toutes les attaches sont futures aussi.
  Seule la classe change à chaque image, jamais la structure.
- **Expansion** (double-clic sur un objet) en deux temps : panneau des
  voisins d'ontologie **comptés par type de lien et type d'objet**, à
  la fenêtre du dossier, hors nœuds déjà affichés (une requête agrégée
  serveur), puis ajout des cases cochées. Les ajouts sont visibles
  jusqu'à « Annuler l'expansion » (pile locale) ou « Garder » (ils vont
  au bac).
- Garde-fous : 300 nœuds affichés au plus (« 42 masqués » + regroupement
  par type), étiquettes d'arêtes au survol ou à la sélection, pas de 3D.

### K32 — Inspecteur à trois modes

- **Sans sélection : « Le dossier à 14:32 »** :
  - événements survenus et en cours à `t` ;
  - où sont les acteurs (zone, ou « pas de position » en rouge pendant
    un trou de signal) ;
  - valeurs des mesures au curseur ;
  - propositions en attente, avec leurs seuils.
- **Événement** :
  - titre éditable sur place ;
  - quand (instant ou période, bascule) et précision ;
  - lieu, avec sa provenance ;
  - pièces attachées avec un sélecteur de rôle, détacher, une zone de
    dépôt, et un champ « attacher un objet » à la frappe (équivalent
    clavier du glisser) ;
  - note d'analyse ;
  - voir sur la carte, retirer du dossier (confirmation : les pièces
    restent).
- **Objet** :
  - type et source ;
  - position et zone à `t`, plage couverte par ses données ;
  - « Suivre uniquement sa chronologie » ;
  - événements du dossier où il intervient, avec son rôle ;
  - relations connues ;
  - lien vers sa page.

### K33 — Cadre, gel, export

- **Cadre** (tiroir) :
  - question à élucider ;
  - fenêtre temporelle **calculée** à partir des événements (plus de
    saisie de fenêtre) ;
  - base de traitement ;
  - seuils des détecteurs ;
  - fuseau d'affichage ;
  - liste des états gelés ;
  - accès aux pages du dossier, au tableau des hypothèses (`cases.md`
    K8) et à la purge (K16) : les fonctions existantes descendent dans
    ce tiroir, elles ne disparaissent pas.
- **Geler l'état** = une ligne `case_snapshots` (§7) :
  - un JSON **canonique** (RFC 8785) du dossier, de ses événements avec
    leurs attaches **résolues** (titre, type, rôle au moment du gel), du
    bac, des propositions retenues ou rejetées, et des **traces
    généralisées** de la fenêtre (bornées à 2 Mo) ;
  - empreinte SHA-256, numéro croissant, libellé, auteur.
  - Plusieurs gels possibles ; un gel ne se modifie jamais (refus
    serveur). L'export d'un gel se relit à l'identique même après
    expiration des données O2.
- **Exporter** : JSON (schéma versionné `pnex.case/1`) et Markdown
  lisible (chronologie, pièces, notes, gel et empreinte), depuis l'état
  courant ou un gel. Un JSON exporté se **réimporte** dans un nouveau
  dossier (test d'aller-retour, piège IRIS). Avertissement si l'export
  contient des personnes (K16).
- L'export à un instant passé ne passe plus par le rejeu du journal O2
  (`cases.md` §12) : on exporte l'état courant ou un gel. Le rejeu reste
  disponible pour l'assistant.

### K34 — Glisser-déposer et palette

- **Palette** à gauche : recherche à la frappe sur tous les objets de
  l'org (point de requête D185), objets groupés par type, membres
  marqués du nombre d'événements où ils interviennent ; « Créer une
  note ou une hypothèse ».
- Le glisser se fait aux **événements pointeur** (patron des éditeurs
  flow et dashboard : capture, seuil de déplacement), pas en HTML5
  `draggable`, qui est imprévisible au tactile et en wasm. Cibles :
  - un événement : attache ;
  - la ligne Événements à `t` : crée l'événement et attache ;
  - la ligne d'un acteur à `t` : crée l'événement avec l'acteur déjà
    attaché ;
  - le bac : range sans date ;
  - la zone de dépôt de l'inspecteur.
- **Équivalent clavier** pour chaque glisser (accessibilité) : champ
  « attacher un objet » de l'inspecteur, menu « Ajouter au dossier »
  hors de l'écran (existant, `AddToCase`).
- Nouveau type système `note` (titre, corps 4 000 car.) : la pièce
  d'analyse minimale (« Vanne V12 restée fermée ? ») sans pack. Le
  `hypothesis` du pack garde le travail d'affirmations sourcées.

### K35 — Écritures unitaires, versionnées, idempotentes

- Chaque geste = une écriture : déplacer un événement = `PUT` de
  l'objet avec `expected_version` (409 → toast « modifié par
  quelqu'un d'autre » + rechargement de l'événement seul) ; attacher =
  ouvrir un lien ; changer de rôle = mettre à jour l'attribut du lien.
  Aucun état d'écran n'est enregistré en bloc (B6).
- Interface optimiste : la frise bouge tout de suite, revient en arrière
  si le serveur refuse.
- Le journal O2 des changements écrit **par lots** (un appel par lot au
  lieu d'un par changement) : retenir 30 détections ou importer ne doit
  pas créer 60 appels séquentiels (extension, §9).

### K36 — Raccourcis clavier

| Touche | Action |
|---|---|
| Espace | Lecture / pause |
| ← / → | Événement précédent / suivant (curseur + sélection) |
| Maj + ← / → | Curseur ± 1 graduation |
| + / − | Zoom de la frise autour du curseur |
| 0 | Ajuster la frise au dossier |
| Échap | Fermer le tiroir, sinon désélectionner, sinon arrêter de suivre |
| R | Retenir la proposition sélectionnée |
| N | Nouvel événement au curseur |

Écouteur unique au niveau du document (patron
`window.__pnexMenuDismiss` de l'`EditorShell`), inactif quand le focus
est dans un champ de saisie. Une aide (« ? ») liste les raccourcis.

### K37 — États vides qui guident

| État | Ce qu'on montre | Action proposée |
|---|---|---|
| Dossier neuf | Frise vide centrée sur maintenant, palette ouverte | « Cliquez sur la frise pour poser le premier événement, ou glissez un objet dessus » |
| Événement sans pièce | « Aucune pièce pour l'instant » | Zone de dépôt + champ « attacher un objet » |
| Aucun acteur suivi | Ligne fantôme sous les événements | « Attachez un device, un véhicule ou une personne qui a une trace » / « Importer une trace » |
| Pas de fond de carte | Carte sur fond uni avec trajets | Lien vers les réglages géo (admin) |
| Aucune détection | « Rien à proposer sur cette fenêtre » | Seuils et « Recalculer » |
| O2 indisponible | Frise et événements intacts, lignes d'acteurs et signaux grisés « données temporelles indisponibles » | Réessayer — jamais d'erreur bloquante pour toute la page |

### K38 — Données personnelles

- **Base de traitement obligatoire avant le premier attachement d'une
  personne**, vérifiée **côté serveur** (409 `case-legal-basis-required`
  à l'attache d'un objet `person` à un événement ou au bac d'un dossier
  sans `legal_basis`). L'UI ouvre le Cadre sur le champ.
- **Journal d'accès** : toute lecture de traces qui concerne une
  personne écrit une entrée dans le stream O2 `case_reads` (qui, quel
  dossier, quels objets, quelle fenêtre). Opt-in par dossier ailleurs
  (`cases.md` K9).
- Purge (`cases.md` K16) étendue aux points importés : purger une
  personne supprime ses traces importées (`delete_stream_range` de son
  origine) ; les positions de devices restent à l'org.

### K39 — Accessibilité, thèmes, téléphone

- Clavier complet (K34, K36), focus visible, rôles ARIA (frise =
  `application` avec instructions, événements = boutons nommés « titre,
  heure »), `prefers-reduced-motion` coupe les pulsations.
- Clair et sombre aux jetons PNEX (`design-system.md`), y compris le
  SVG du graphe et de la frise (`var(--color-…, #hex)`).
- **Téléphone = consultation** : frise compacte en haut, une vue à la
  fois (carte, graphe, inspecteur) en onglets, lecture et sélection ;
  pas de glisser, l'attache passe par l'inspecteur.

### K40 — Vues commutables

Carte / Côte à côte / Graphe (segmenté, mémorisé par utilisateur en
stockage local) ; le bac reste sous la vue. Réponse au risque de
surcharge (§13), à valider au test d'usage.

### K41 — Nom

Proposition issue du benchmark (§15, Q3) : navigation **« Enquêtes »**
(fr) / **« Investigations »** (en), titres longs « dossier d'enquête »,
type système `case` inchangé. Cela lève la collision avec « Dossier »
= `folder`. À trancher par Shan.

### K42 — Enquête média : capter, horodater, comparer, garder la chronologie

La même enquête sert le travail documentaire sur des émissions captées
en direct ou en différé (radio, télévision, débats publics). Le concept
tient en quatre gestes :
1. **capter** ce qui se dit à l'antenne ;
2. **horodater** chaque déclaration à l'heure de diffusion ;
3. **proposer** les rapprochements utiles : positions qui évoluent,
   jugements qui diffèrent sur des situations comparables, écarts avec
   un fait documenté ;
4. **garder la chronologie des faits** à côté de ce qui a été dit.

Rien de neuf dans l'écran : la frise, les lignes, les signaux, les
propositions et l'inspecteur servent tels quels. Seules les sources et
les détecteurs changent.

| Enquête RCA | Enquête média |
|---|---|
| Ligne d'acteur = zones de présence (traces) | Ligne de personne = **apparitions** (`cases.md` K17) : en plateau, temps de parole, mentionnée, à l'heure de diffusion, et ses **déclarations** (K43) |
| Trou de signal | Période sans apparition captée (flux non suivi ≠ absence : la ligne le dit) |
| Signaux = alertes, changements | Signaux = déclarations captées non retenues, mentions par alias, grilles d'émission (plages D169) |
| Détections géométriques (K30) | Détections de discours (K44) |
| Carte | **Chronologie des citations** : liste datée des déclarations, filtrée par personne et par sujet, chacune ouvrable à la seconde |

L'échelle de la frise passe de la seconde aux années (K23 la couvre déjà) :
une enquête média s'étale souvent sur des mois, et une déclaration se
lit à la seconde.

### K43 — Déclaration : un objet daté, cité mot pour mot, rattaché à sa source

- Type **`statement`** (pack `investigation` 1.4.0) :
  - `quote` : texte **verbatim**, extrait de la transcription, jamais
    reformulé ;
  - `aired` : `fuzzy_time`, heure de diffusion (`cases.md` K17 : deux
    horloges, diffusion et position dans l'enregistrement) ;
  - `locator` : flux ou média, segment, décalage en ms ;
  - `topics` : sujets, taxonomie D168 ;
  - `stance` : `for` | `against` | `nuanced` | `none` ;
  - `context_before` / `context_after` : deux phrases de part et
    d'autre, pour ne jamais citer hors contexte.
- Liens :
  - `said_by` : déclaration → personne. Attribution par grille,
    présentation à l'antenne, voix enrôlée avec consentement (K19) ou
    main humaine ; `origin` et `confidence` sont portés par le lien ;
  - `about` : déclaration → objet visé (personne, organisation, projet) ;
  - `supports` / `contradicts` : existent déjà.
- **Capter** : un flow déployé par l'humain (D123), avec le nouveau nœud
  `statement_extract`, découpe les segments transcrits (`tx_<slug>`, D165)
  des personnes suivies en déclarations candidates. Il les écrit dans un
  stream O2 **`statements`** (horodatées, verbatim, locator) : c'est le
  brut, et rien n'est encore un objet (ancre paresseuse, B1).
  - Le découpage en phrases et l'attribution par la grille sont
    déterministes.
  - La position (`stance`) et les sujets sont **proposés** par le LLM de
    l'org quand il existe, en classifieur fermé (D168 : sortie dans une
    liste fermée, texte passé comme donnée non fiable). Sans LLM : sujets
    par mots-clés (D168, existant) et position laissée vide.
  - Le runtime de flows n'a pas encore d'accès au LLM de l'org
    (`media-ingest.md`, lot 2) : en attendant, la classification se fait
    côté serveur, au moment où l'enquête lit les déclarations (même règle
    de classifieur fermé).
- **Retenir** une déclaration (signal → objet) se fait en une
  transaction :
  - crée l'objet `statement` et son `said_by` ;
  - l'attache à un événement du dossier, sinon au bac ;
  - **épingle la preuve** : l'extrait audio ou vidéo (± 30 s) est copié
    en médiathèque avec son SHA-256, s'il est encore disponible (la
    rétention audio est nulle par défaut, D161). Sinon la déclaration
    est marquée « transcription seule, extrait non conservé ». Un flux
    suivi pour une enquête devrait avoir une rétention glissante
    (réglage du flux, D161), et l'UI le rappelle.
- Une déclaration n'est attribuée à une personne que si l'attribution
  existe. Sinon elle reste « locuteur S3, non attribué », à attribuer à
  la main. Jamais de reconnaissance faciale ; la voix seulement par
  enrôlement consenti (K19).

### K44 — Détecteurs de discours : proposés, sourcés, jamais conclus

Trois détecteurs alimentent `case_proposals` (même cycle que K30 :
proposé → retenu | écarté, empreinte, jamais reproposé après rejet) :

| Détecteur | Ce qu'il cherche | Comment |
|---|---|---|
| **Positions opposées** | Une même personne, un même sujet, deux positions opposées (`for` ↔ `against`) à deux dates ou devant deux publics | Paires de déclarations de la personne sur un sujet commun ; position comparée. Le LLM de l'org, s'il est configuré, confirme que les deux phrases portent bien sur la **même proposition** et rend une justification courte. Sans LLM, seules les paires dont les positions sont validées à la main sont proposées |
| **Jugements différents de situations comparables** | Une même personne juge deux situations comparables de façon opposée selon qui est concerné | Paires de déclarations de la même personne, de même critère (sujet, ex. « conflit d'intérêts »), dont les **cibles** (`about`) diffèrent et dont le jugement s'oppose. L'**ontologie qualifie la relation** à chaque cible (`member_of` le même parti, `knows`, `employs`…) : « adversaire » / « allié » apparaît dans la proposition comme un **fait sourcé par des liens**, jamais comme un jugement |
| **Écart avec un fait sourcé** | Une déclaration contredit une affirmation (`claim`) déjà corroborée dans le dossier (K7) | Déclarations et affirmations sur le même sujet ; LLM en classifieur fermé `contradicts | consistent | unrelated` ; la proposition montre la déclaration et le fait avec ses pièces |

Garde-fous du calcul (opposables) :
- **Vérification mot pour mot côté serveur** : toute citation d'une
  proposition doit se retrouver telle quelle dans la transcription source
  (comparaison normalisée des espaces et de la ponctuation). Sinon la
  proposition est **jetée**. Un LLM qui reformule ou invente ne peut
  rien proposer.
- La justification du LLM est affichée comme telle (« explication
  proposée par l'assistant »), jamais comme un constat ; elle ne passe
  jamais dans un export sans avoir été relue (champ éditable à la
  rétention).
- Calcul déclenché par l'humain (« Recalculer », ouverture du dossier),
  borné par dossier (nombre de paires, budget de tokens de l'org) ; texte
  des transcriptions passé comme **donnée non fiable** (injection de
  prompt), sortie dans une liste fermée.

> Prolongement : formules récurrentes entre locuteurs et médias, mesures
> de pluralisme et indices de coordination dans `discourse-analysis.md`
> (DA1–DA10 ; composant à placer, proposé d'abord comme outil de
> l'assistant appelé depuis une note de l'enquête).

### K45 — Vocabulaire neutre, contexte complet, droit de réponse

Le produit ne qualifie jamais une personne ; il montre des faits datés et
leur source.

- Libellés imposés : « Positions opposées », « Jugements différents de
  situations comparables », « Écart avec un fait sourcé ». Le produit ne
  pose aucun qualificatif sur une personne ni sur ses intentions.
- Chaque citation s'affiche avec ses deux phrases de contexte, et
  s'ouvre à la seconde dans l'enregistrement (s'il est conservé). Le
  comparateur met les deux citations **côte à côte**, avec date, heure,
  émission, attribution et sa confiance.
- Écarter une proposition demande un motif :
  - « pas le même sujet » ;
  - « situations non comparables » ;
  - « évolution expliquée » (la personne a justifié son changement) ;
  - « attribution douteuse » ;
  - autre, à préciser.
  Le motif reste attaché à la proposition (journal) : on sait pourquoi on
  n'a pas retenu.
- Un événement retenu de ce type porte une case **« réponse de la
  personne »** : lien vers sa déclaration ultérieure ou note de prise de
  contact. L'export Markdown la fait apparaître à côté de la proposition.
- Base de traitement (K38) : pour une org de presse, « exercice
  journalistique (art. 85 RGPD, art. 80 loi Informatique et Libertés) »
  ; personnalités publiques dans leur rôle public. Aucune publication
  depuis PNEX (D173) : l'export est un dossier de travail.

### K46 — Chronologie des faits et des déclarations, séparées et alignées

- La ligne **Événements** porte les **faits** (nomination, signature d'un
  contrat, vote), sourcés par des documents. Les lignes de personnes
  portent ce qu'elles **ont dit**. On voit sur une seule frise qu'une
  position a changé **après** tel fait (« contre » en mars, nommée en
  juin, « pour » en septembre). C'est le cœur du travail.
- Le panneau **Chronologie des citations** remplace la carte dans une
  enquête sans traces. Il liste les déclarations par date, filtrées par
  personne suivie et par sujet. Le curseur atténue ce qui vient après,
  et un clic ouvre la citation.
- Les propositions K44 s'affichent sur la ligne Signaux comme une
  période (première → seconde déclaration). Retenue, elle devient un
  événement période, avec les deux déclarations attachées en Preuve et
  la personne en Acteur.

### K47 — Fond commun avec l'enquête RCA

Une enquête peut mêler les deux : une inspection d'usine et la
couverture médiatique de l'accident. Il n'y a pas de « mode » à choisir.
Les panneaux suivent les membres :
- carte si au moins un membre a une trace ;
- chronologie des citations si au moins un membre a des déclarations ;
- les deux en onglets s'il y a les deux.

## 7. Modèle de données

Deux tables nouvelles, le reste dans l'ontologie :

| Élément | Où | Nouveau |
|---|---|---|
| `case` | type système sans table (existant) ; gagne `display_tz`, `copresence_distance_m`, `copresence_min_s`, `stop_diameter_m`, `stop_min_s`, `signal_gap_s` ; `window_from`/`window_to` deviennent dérivés (lecture) | propriétés |
| `event` | type système sans table (K20) | type noyau (repris du pack) |
| `note` | type système sans table (K34) | type noyau |
| `involves` | lien système `event` → `*`, `role` enum + `detail` (K21) | lien noyau (repris du pack) |
| `in_case` | existant ; sert aux événements du dossier et au bac (K22) | sens précisé |
| `tracks` | lien système temporel device → `*` (K25) | lien noyau |
| `geo_area` | type de propriété `pnex-core` (K29) | oui |
| `case_proposals` | table Postgres (K30) | **oui** |
| `case_snapshots` | table Postgres (K33) | **oui** |
| O2 `positions` | stream logs (K26) | oui |
| O2 `case_reads` | stream logs (K38 ; prévu par `cases.md` K9) | oui |
| `statement` + liens `said_by`, `about` | pack `investigation` 1.4.0 (K43) | type et liens (pack) |
| O2 `statements` | stream logs des déclarations captées (K43) | oui |
| Nœud `statement_extract` | runtime de flows, `NodeDoc` (K43) | nœud |
| `case_proposals.kind` | + `opposed_positions`, `double_standard`, `fact_gap` (K44) ; `reject_reason` (K45) | valeurs + colonne |

```sql
CREATE TABLE case_proposals (
  id           uuid PRIMARY KEY,
  org_id       bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  case_id      uuid   NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
  kind         text   NOT NULL,          -- co_presence | zone_enter | zone_exit | stop | signal_loss | opposed_positions | double_standard | fact_gap
  detector     text   NOT NULL,          -- "<kind>@<version>"
  params       jsonb  NOT NULL,          -- seuils effectifs du calcul
  fingerprint  text   NOT NULL,          -- sha256(kind, refs triés, t_start arrondi)
  t_start      timestamptz NOT NULL,
  t_end        timestamptz,
  refs         uuid[] NOT NULL,          -- objets concernés (acteurs, zone)
  payload      jsonb  NOT NULL,          -- lieu, distance min, qualité observée/interpolée…
  status       text   NOT NULL DEFAULT 'proposed', -- proposed | retained | rejected
  reject_reason text,                   -- K45 : motif d'écart (journalisé)
  event_id     uuid REFERENCES objects(id) ON DELETE SET NULL,
  reviewed_by  bigint REFERENCES users(id),
  reviewed_at  timestamptz,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (case_id, fingerprint)
);
CREATE INDEX ON case_proposals (case_id, status, t_start);

CREATE TABLE case_snapshots (
  id          uuid PRIMARY KEY,
  org_id      bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
  case_id     uuid   NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
  seq         integer NOT NULL,
  label       text,
  taken_by    bigint REFERENCES users(id),
  taken_at    timestamptz NOT NULL DEFAULT now(),
  content     jsonb  NOT NULL,           -- JSON canonique RFC 8785
  sha256      char(64) NOT NULL,         -- empreinte des octets canoniques
  size_bytes  integer NOT NULL,
  UNIQUE (case_id, seq)
);
```

- `org_id` sur chaque ligne et **toute requête filtrée par l'org du
  principal** (R1) ; suppression du dossier = cascade.
- `frozen_at` (propriété actuelle) devient l'instant du dernier gel, en
  lecture ; la migration crée un gel n° 1 pour un dossier déjà gelé.
- Pas de table `case_event` ni `case_attachment` (v0.1) : événements et
  attaches sont des objets et des liens (K20, K21). Pas de table
  `case_undated` : le bac se déduit (K22).

## 8. API (additive)

| Route | Rôle | Garde |
|---|---|---|
| `GET /api/v1/cases/{id}/workspace` | Un seul chargement initial : dossier, événements avec attaches, bac, acteurs suivis (avec leur source de trace), mesures, propositions en attente, fenêtre dérivée | lecture org |
| `POST /api/v1/cases/{id}/events` | Créer un événement **et** ses attaches initiales en une transaction (dépôt d'un objet sur la frise) | écriture `event` (R2) |
| `POST /api/v1/cases/{id}/attach` / `…/detach` | Attacher un objet à un événement, ou le ranger au bac ; détacher. Intention explicite, idempotente | écriture (R2), K38 |
| `POST /api/v1/cases/{id}/tracks` | Traces de la fenêtre (K27) | lecture org, journal K38 |
| `POST /api/v1/cases/{id}/signals` | Signaux bruts de la fenêtre (chronologie actuelle, refondue) | lecture org |
| `POST /api/v1/cases/{id}/detect` | Recalcul des détections (K30) | écriture (R2) |
| `GET /api/v1/cases/{id}/proposals` | Propositions par statut | lecture org |
| `POST /api/v1/cases/{id}/proposals/{pid}/retain` / `…/reject` | Intentions explicites, idempotentes | écriture (R2) |
| `POST /api/v1/cases/{id}/snapshots` · `GET …/snapshots` · `GET …/snapshots/{seq}` | Geler, lister, relire | écriture (R2) / lecture |
| `POST /api/v1/cases/{id}/export` | JSON `pnex.case/1` ou Markdown, courant ou gel | lecture org |
| `POST /api/v1/cases/import` | Réimport d'un export JSON | écriture (R2) |
| `POST /api/v1/tracks/import` (+ aperçu) | Import de trace (K26) | écriture (R2) |
| `POST /api/v1/cases/{id}/statements` | Déclarations captées de la fenêtre (stream `statements`), par personne suivie et sujet (K43, K46) | lecture org |
| `POST /api/v1/cases/{id}/statements/retain` | Retenir une déclaration : objet, `said_by`, attache, preuve épinglée, en une transaction (K43) | écriture (R2) |
| `POST /api/v1/ontology/objects/{id}/expand` | Facettes comptées puis voisins choisis (K31), `as_of` | lecture org |

- Le reste (édition d'événement, de note, de rôle) passe par
  `/api/v1/ontology/*`.
- `POST /cases/{id}/timeline`, `GET /cases/{id}/snapshot` et l'export
  rejoué sont gardés pour l'assistant le temps d'un cycle, puis retirés.
- Chaque route d'écriture : garde de rôle en tête du handler, test viewer
  → 403, org depuis le principal.
- Erreurs : codes machine dans `err_codes::ALL` et clés `err-…` dans les
  deux `.ftl` : `case-legal-basis-required`, `case-snapshot-immutable`,
  `track-import-timezone-required`, `track-import-empty`,
  `case-proposal-stale`, `statement-quote-mismatch`, `statement-unattributed`.

## 9. Règles assouplies ou étendues (accord de Shan du 2026-10-12 : « s'il faut étendre ce qu'on a ou alléger nos règles, faire le nécessaire »)

| Règle d'origine | Où | Changement | Pourquoi |
|---|---|---|---|
| « Aucune table parallèle en V1 » (K1) | `cases.md` | **Deux tables** : `case_proposals`, `case_snapshots` | Les propositions sont un volume calculé avec un cycle de revue, ce ne sont pas des faits. Un gel est un document immuable et empreint. Ni l'un ni l'autre n'est un objet. Le rejeu par le journal O2 ne survit pas à sa rétention |
| « Le noyau ne gagne que `case`, `in_case` et la chronologie » (K4, D190) | `cases.md`, `ontology.md` | Le noyau gagne `event`, `note`, `involves`, `tracks`, `geo_area` | L'événement est l'objet central de **tout** dossier, RCA comprise ; un dossier ne doit pas exiger un pack métier |
| Membres saisis par `in_case` avec rôle (K2) | `cases.md` | Membres **déduits** des attaches ; `in_case` = événements + bac | Rôle choisi à l'attache, dans un fait (B3) |
| Fenêtre du dossier saisie (K1) | `cases.md` | Fenêtre **dérivée** des événements | Moins de formulaire ; elle ne ment jamais |
| Chronologie = liste unifiée (K6) | `cases.md` | Signaux bruts **séparés** des événements retenus | B1 : promouvoir, pas mélanger |
| Dernière position seulement (D38) | `pois` | Chaque position complète part aussi dans O2 `positions` | La carte de trajets et les détecteurs en ont besoin |
| Graphe radial (L4) | `graph_view.rs` | Graphe de dossier à forces déterministes, natif | Biparti objets–événements, estompage, expansion |
| Carte à marqueurs seuls | `map.js` | API de couches et de temps | Trajets, zones, traînées |
| Journal O2 : un appel par changement | `ontology/changes.rs` | Écriture par lots | Retenir ou importer en masse sans retard |
| Séries en préréglages relatifs | `/series/{prop}?window=` | Bornes absolues `from`/`to` | Les lignes de mesures suivent la frise |
| Pas de dépendance géométrique | `Cargo.toml` | `geo`, `geographiclib-rs`, `rstar` (MIT/Apache, `cargo deny`) | Détecteurs, zones, distances |

Ne bougent pas : sécurité R1–R20, consignes de l'assistant (D123 :
l'assistant ne retient, ne gèle ni n'attache rien sans geste humain),
i18n (`t!`, parité des `.ftl`), concurrence optimiste, PostgreSQL seul,
images Chainguard.

## 10. Exigences non fonctionnelles

- Tourne dans la stack existante, **sans nouveau service** ; cible Pi 5.
- Lecture sans requête par image (K27, K28). Le curseur qui bouge ne
  re-rend que le curseur, les repères « passé / futur », les têtes et
  les traînées de la carte, l'estompage du graphe et l'inspecteur. Les
  événements, les lignes et le layout du graphe ne sont recalculés
  qu'au changement de données ou de zoom. Signaux Dioxus fins : un
  signal pour le temps, un pour les données.
- Budgets mesurés (test de charge en L3) : §2.
- Mode dégradé O2 : K37.
- Clavier, thèmes, téléphone : K36, K39.

## 11. Phase 1 — design : ce qu'il faut produire

1. **Jetons PNEX** (`design-system.md`, `style/tailwind.css`) : le
   prototype les remplace par les siens (Fira Sans, accent orange,
   capitales). Les maquettes reprennent IBM Plex, `signal`, `mark`,
   `ok/warn/alarm`, les tailles 12/14/16/20 et les rayons PNEX. Les
   couleurs d'acteurs viennent d'une palette catégorielle validée en
   clair et en sombre (jetons ajoutés à `design-system.md`, pas de
   `blue-*`).
2. **Composants nouveaux**, spécifiés avant d'être codés : frise (règle,
   lignes, curseur, barre de période, hachure de trou), puce de rôle,
   inspecteur à trois modes, tiroir (n'existe pas encore), bac, contrôle
   segmenté (n'existe pas encore), carte de proposition. Toast existant.
3. **Maquettes haute fidélité** (artifact navigable, données
   d'exemple) :
   - dossier vide ;
   - dossier en cours ;
   - lecture en cours ;
   - suivi d'un acteur ;
   - événement sélectionné ;
   - objet sélectionné ;
   - Cadre ;
   - gel et export ;
   - import de trace (aperçu, rejets) ;
   - largeur téléphone ;
   - clair et sombre.
4. **Test d'usage** (3 personnes : IoT/RCA, enquête data, novice),
   tâches chronométrées : poser un événement et y attacher une pièce ;
   suivre un acteur ; retenir une co-présence ; geler et exporter.
5. **Écrans voisins** (Médias, Devices, Pages, Ontologie) : relevé des
   frictions au même design system (recherche d'objet, états vides,
   libellés anglais restants tels que « Investigation note »), corrigées
   avant ou avec L2.

## 12. Lots (phase 3)

| Lot | Contenu | Sortie vérifiable |
|---|---|---|
| **L0** — Socle | Migration (§7) et reprise du pack 1.3.0 ; module `pnex-core::track` (normalisation, échantillonnage, trou de signal, TDTR, ENU) et `geo_area` avec tests portés de MovingPandas ; stream O2 `positions` depuis `GpsTapSink` ; `POST …/tracks` ; journal O2 par lots ; séries à bornes absolues | Traces d'un device réel (banc) servies sur 2 h ; tests de modèle et de contrat verts |
| **L1** — Frise | Règle, zoom, curseur, lignes Événements / acteurs / mesures, création instant et période, déplacement, `…/workspace`, `…/events`, raccourcis, états vides | Événement posé + déplacé ; replay 3 acteurs × 2 h ≥ 30 im/s |
| **L2** — Glisser et attacher | Palette avec recherche, dépôt sur événement, ligne, bac, inspecteur ; `…/attach`, rôles, K38 ; type `note` | < 60 s pour poser et attacher (test d'usage) ; viewer → 403 |
| **L3** — Carte | `map.js` couches + `setTime`, trajets jusqu'au curseur, têtes, zones (cercle dessiné, polygone importé), épingle d'événement, lecture ×1/×3/×8 ; import de trace (aperçu, fuseau, rejets) ; test de charge | Banc : camion et badge rejoués ; GPX importé = même rendu qu'un flux |
| **L4** — Graphe | Layout déterministe `pnex-core`, rendu SVG, estompage, expansion à facettes + annulation, plafond | 300 nœuds fluides ; expansion annulée sans trace serveur |
| **L5** — Détecteurs | Co-présence, zones, arrêts, fin de signal ; `case_proposals`, signaux bruts, retenir / rejeter ; seuils dans le Cadre | Scénario du prototype : les 5 détections attendues, aucune en double au recalcul, rejet non reproposé |
| **L6** — Cadre, gel, export | Tiroir Cadre (pages, tableau, purge descendus), gels multiples, JSON + Markdown, réimport, aide clavier ; assistant et fiche KB mis à jour | Gel relu à l'identique après purge O2 ; aller-retour export → import identique |
| **L7** — Enquête média | Dépend de `cases.md` C8 (apparitions) et des lots média 1–3 : type `statement`, stream `statements`, nœud `statement_extract`, retenir une déclaration (preuve audio épinglée), chronologie des citations, détecteurs K44 avec vérification mot pour mot, comparateur, motifs d'écart, réponse de la personne | Débat capté en direct : une déclaration retenue s'ouvre à la seconde ; une paire « positions opposées » proposée, aucune citation non verbatim acceptée (test qui injecte une reformulation) |

Retraits au fil des lots : formulaire d'en-tête, panneau membres par
rôle, composeur de lien, chronologie verticale, panneau « Le dossier à un
instant » (`workspace.rs`, `snapshot.rs`) ; outils assistant
`case_timeline` et `case_snapshot` réécrits sur les nouveaux services
(règle CLAUDE.md : assistant livré dans le même commit).

## 13. Risques

| Risque | Parade |
|---|---|
| Surcharge visuelle (3 vues + frise) | Vues commutables (K40), signaux repliables, validé au test d'usage |
| Volume de points GPS | TDTR selon le zoom, plafond par réponse, cache de fenêtres (K27) |
| Faux positifs de co-présence | Seuils par dossier, hystérésis, « inconnu » pendant un trou, statut proposé (K30) |
| Citation hors contexte, rapprochement abusif | Contexte affiché et exporté, vérification mot pour mot, motif d'écart, vocabulaire neutre, réponse de la personne (K44, K45) |
| LLM qui reformule ou invente | Proposition jetée si une citation n'est pas verbatim ; justification marquée « proposée par l'assistant », relue avant export (K44) |
| Attribution erronée d'une déclaration | Attribution avec origine et confiance ; non attribuée par défaut ; ni visage ni voix non consentie (K43) |
| Preuve perdue (audio non conservé) | Extrait épinglé à la rétention, avertissement si la rétention du flux est nulle (K43) |
| Données personnelles (trajets de personnes) | Base de traitement vérifiée côté serveur, journal d'accès, purge des traces importées (K38) |
| Deux implémentations de l'échantillonnage (Rust, JS) | Fixtures communes exécutées des deux côtés (K28) |
| Le journal O2 retarde les écritures en masse | Écriture par lots (K35) |
| Fuseaux et dates naïves | UTC stocké, fuseau d'affichage étiqueté, fuseau obligatoire à l'import (K23, K26) |
| Refonte d'un écran déjà livré (C0–C7) | Les services restent (chronologie → signaux, pages, tableau, purge) ; seul l'écran est remplacé, lot par lot |
| Collision avec la refonte UI en cours (autre session) | Composants dans `components::ui`, jetons du design system ; coordination avant de toucher `ui.rs` |

## 14. Critères d'acceptation transverses

- Toute action de l'écran a son équivalent clavier ; `i18n_guard` et
  `design_guard` verts ; parité des `.ftl`.
- e2e Playwright :
  - scénario RCA (prototype) ;
  - scénario « données seules » (GPX importé, aucun device) ;
  - état vide ;
  - téléphone ;
  - viewer en lecture seule.
- `/security-review` avant merge ; lignes SEC-n pour tout écart.

## 15. Questions ouvertes de la v0.1 — réponses

1. **Traces : OpenObserve seul ou adaptateur ?** Les deux, par le même
   stream (K26) : les flux devices y écrivent et les imports GPX,
   GeoJSON ou CSV y sont ingérés une fois, sous leur origine. Une seule
   lecture (K27).
2. **Graphe natif ou Cytoscape.js ?** Natif (K31) : cinq projets sur six
   montrent que l'usage est modeste et que les problèmes viennent du
   volume. Un layout déterministe en Rust et un rendu SVG suffisent à
   l'échelle d'un dossier. Cytoscape reste en réserve pour des layouts
   hiérarchiques.
3. **Nom ?** « Enquêtes » en navigation, « dossier d'enquête » en titre
   long (K41), à trancher.

## 16. Questions nouvelles (à trancher)

- **Q-T1** — Temps de l'attache : la fermeture d'un lien `involves` se
  date au temps système (« détaché le 12/10 »). Le temps du monde vit
  dans l'événement. Faut-il un jour une validité monde sur l'attache
  (« X présent de 14:02 à 14:20 dans un événement de 2 h ») ? Proposé :
  non en v1 ; une sous-période devient un événement.
- **Q-T2** — Positions historiques antérieures au stream `positions` :
  faut-il un rattrapage unique qui apparie les séries `latitude` et
  `longitude` d'O2, pour les devices du banc et de dev.pnex.io ?
  Proposé : non, sauf besoin de démo.
- **Q-T3** — Palette de couleurs d'acteurs : 8 couleurs catégorielles
  suffisent-elles (au-delà, la couleur se répète avec les initiales) ?
- **Q-T4** — Détections relancées automatiquement quand une trace
  change (nouveaux points d'un device en direct), ou seulement à
  l'ouverture et sur « Recalculer » ? Proposé : à la demande en v1.

## 17. Avancement

### 2026-10-12 — socle L0, frise L1, glisser L2 (non commité)

Beta : aucune donnée existante n'est préservée (accord de Shan). L'ancien
écran et ce qu'il portait sont retirés, pas mis en cohabitation.

| Élément | Où | État |
|---|---|---|
| Module de traces (K26–K28) : normalisation avec rejets motivés, découpage au trou de signal (`>` strict), échantillonnage sans interpolation à travers un trou, repère local en mètres, TDTR | `pnex-core/src/track.rs` | code + tests |
| Noyau : `event` (`occurred`, `note`, `origin`, `location`), `note` (`body`), `involves` (`role` fermé + `detail`), `tracks` (device → objet) ; `in_case` sans attribut ; `case` sans fenêtre saisie, modèle ni « gelé le » | `pnex-core/src/ontology/mod.rs` | code |
| Pack `investigation` 1.3.0 (n'embarque plus `event` ni `involves`) | `packs/investigation.yaml` | code |
| Tables `case_proposals`, `case_snapshots` | migration `m20261012_000013_case_workspace` | schéma, sans service |
| Historique des positions (stream O2 `positions`, par lots, sans bloquer la télémétrie) | `services/pois/position_log.rs` | code + test ; vérifié sur O2 réel |
| Service des dossiers réorganisé : `services/ontology/cases/` (appartenance dérivée, espace de travail, attacher avec changement de rôle, détacher, créer un événement, ouverture depuis une alerte = premier événement), `board`, `export` (`pnex.case/1`), `pages`, `purge` (« détenu » = attaché à ses événements ou non daté), `signals` (ex-chronologie, sans les opérations de l'espace de travail), `tracks` | `crates/pnex-backend/src/services/ontology/cases/` | code + tests d'intégration |
| API : `GET workspace`, `POST events`, `POST attach`, `POST detach`, `POST signals`, `POST tracks`, `GET board`, `GET export` ; retirés : `timeline`, `snapshot`, export rejoué | `controllers/cases.rs` | code |
| Erreur `case-legal-basis-required` (K38, vérifiée côté serveur) | `err_codes`, `.ftl` | code + test |
| Assistant : `case_workspace`, `case_signals`, `case_board` (retirés : `case_timeline`, `case_snapshot`) ; fiche réécrite | `services/ai/`, `assistant-kb/cases.md` | code + tests |
| Écran : frise (règle, densité, Événements clic/glisser/déplacer, Signaux, une ligne par membre suivi avec trous de signal, curseur, lecture ×1/×3/×8/ajustée, UTC ↔ heure locale, zoom), palette avec recherche et glisser-déposer pointeur (événement, ligne d'acteur, bac), bac « Non daté », inspecteur trois modes (rôles, détacher, attacher au clavier, note, retirer), carte (positions au curseur), graphe (vue existante), Cadre (question, base de traitement, fenêtre dérivée, tableau des hypothèses, pages, purge), export JSON | `pnex-frontend/src/pages/cases/workspace/` | code ; vu en clair, sombre et 390 px sur un serveur de test, avec traces simulées |
| e2e réécrit (événement au clic, glisser-déposer, rôle, détacher, ajout depuis une page objet, ouverture depuis une alerte, pages au Cadre, purge) | `e2e/tests/cases.spec.ts` | 5/5 verts |

**Constat à traiter (K26)** : O2 refuse l'ingestion de points plus vieux
que 168 h (`ZO_INGEST_ALLOWED_UPTO`). Sans effet sur les positions en
direct, mais l'import d'une trace ancienne (GPX de plusieurs mois) serait
rejeté. Avant L3 : soit relever ce plafond dans le déploiement (et le
documenter), soit stocker les traces importées hors d'O2 (table dédiée ou
Parquet par import). À trancher.

### 2026-10-12 (suite) — carte L3 (partiel), détecteurs L5 (non commité)

| Élément | Où | État |
|---|---|---|
| `map.js` : `setTracks` (une fois par fenêtre) et `setTime` (le curseur seul) ; trajet complet en pointillé, passé plein, dernier dixième de la fenêtre plus marqué ; rien à travers un trou de signal ; cadrage `fitBounds` sur la taille réelle de la carte | `js/map.js`, `js/track_cut.js` (+ `track_cut.test.js`, `bun test js/`), `map_viewer.rs` | code + tests ; vu sur serveur de test |
| Épingle de l'événement sélectionné (propriété `location`) | `workspace/map.rs` | code |
| Détecteurs purs : co-présence (40 m, ≥ 2 min, sortie à 1,2 ×, inconnue pendant un trou), arrêts (25 m, ≥ 15 min, fenêtre glissante, point médian), pertes de signal (trou ou trace qui se tait avant la fin de la fenêtre) | `pnex-core/src/track/detect.rs` | code + tests |
| Lecture des traces factorisée (`load`) entre `tracks` et les détecteurs | `cases/tracks.rs` | code |
| Propositions : `POST detect` (écriture, R2), `GET proposals`, `POST proposals/{pid}/retain` (événement `origin = detection`, membres en *acteur*, lieu, détecteur et seuils en note, `source_ref = detector:<clé>@<version>`, idempotent sous verrou de ligne), `POST proposals/{pid}/reject` (motif optionnel, jamais reproposé) ; empreinte = détecteur, membres triés, début à la minute | `cases/detect.rs`, `controllers/cases.rs` | code + test d'intégration |
| Erreur `case-proposal-stale` (retenir un rejet, rejeter un retenu) | `err_codes`, `.ftl` | code + test |
| Assistant : `case_proposals` (lecture seule) ; fiche mise à jour | `services/ai/`, `assistant-kb/cases.md` | code + test |
| Écran : détections en attente sur la ligne Signaux (bord plein), section *Détections* de l'inspecteur (Retenir / Écarter), bouton *Recalculer* ; calcul à l'ouverture pour qui peut écrire | `workspace/` | code ; scénario de démo : 6 détections attendues (arrêt au quai, co-présence 8,6 m, trou du badge, arrêt du badge, fins de trace), retenue → 3ᵉ événement |

Écarts assumés, notés en code (`shortcut:`) : seuils par défaut pour tous
les dossiers (les propriétés du Cadre restent à poser) ; bornes de
co-présence à la résolution des points (pas de résolution quadratique
entre deux points) ; arrêt mesuré par la diagonale de la boîte englobante
(plus strict que le diamètre vrai) ; empreinte arrondie à la minute (un
début à cheval sur une minute peut se dédoubler après un recalcul sur une
autre fenêtre). Le test e2e des détections attend un moyen d'injecter des
positions dans l'O2 de test.

### 2026-10-12 (suite) — zones et import de traces (non commité)

**Décision K26 (tranchée)** : les traces importées vivent dans **Postgres**
(`track_imports`, `track_points`), pas dans O2. Le plafond d'ingestion
d'O2 (`ZO_INGEST_ALLOWED_UPTO`, 5 h par défaut, relevé à 168 h dans
`compose.yaml` pour la file hors ligne de l'agent edge D95) n'est pas la
vraie raison : même relevé, une trace de plusieurs mois tomberait sous la
**rétention de télémétrie** (D72) et serait purgée avec elle. Une trace
importée est une pièce du dossier : elle suit l'objet (cascade à la
purge), garde l'empreinte SHA-256 du fichier, et n'a pas de limite d'âge.
O2 reste la source des positions en direct ; la lecture `load` fusionne
les deux.

| Élément | Où | État |
|---|---|---|
| Type de propriété `geo_area` (cercle ≤ 100 km ou polygone GeoJSON à un anneau fermé, ≤ 1 000 sommets, sans croisement), point-dans-zone en repère métrique, rendu en polygone | `pnex-core/src/geo_area.rs`, `ontology/schema.rs` | code + tests |
| `place` gagne `zone` (pack `investigation` 1.4.0 ; `statement` passera donc en 1.5.0) ; tout membre portant une valeur `geo_area` est une zone | `packs/investigation.yaml`, `cases/mod.rs` (`zones_of`) | code |
| Détecteur `zone@1` : visites par segment, entrée et sortie interpolées par dichotomie, une visite coupée par un trou devient deux (bords non observés signalés) ; à la retenue, la zone est attachée en *contexte* | `track/detect.rs`, `cases/detect.rs` | code + tests |
| Zones des membres dans `GET workspace` (`zones`) et sur la carte (`setZones`), cadrage sur traces et zones | `api.rs`, `map.js`, `workspace/map.rs` | code ; vu sur serveur de test |
| Saisie `geo_area` : cercle (latitude, longitude, rayon) ou polygone GeoJSON collé (Polygon ou Feature) ; type proposé dans l'éditeur de types | `components/ontology/property_input.rs`, `type_editor.rs` | code |
| Lecture GPX (`trkpt`, `ele`, `time`) et CSV (colonnes détectées, `,` `;` tab, virgule décimale avec `;`, epoch s/ms), heures sans décalage → fuseau IANA exigé (heure inexistante au changement d'heure = ligne écartée, ambiguë = la plus tôt) ; chaque ligne écartée a son motif | `services/pois/track_import.rs` | code + tests |
| Import : `POST /cases/{id}/tracks/import` (aperçu `dry_run`, puis écriture ; membre du dossier seulement ; K38 pour une personne ; 20 Mo, 500 000 points ; même fichier sur le même membre = même import), `GET …/tracks/imports`, `DELETE …/tracks/imports/{iid}` ; erreurs `track-import-timezone-required`, `track-import-empty`, `track-import-unreadable` | `cases/imports.rs`, `controllers/cases.rs`, migration 000013 | code + test d'intégration (aperçu, fuseau, idempotence, traces servies sans O2, zone + arrêt détectés, aucun doublon au recalcul, R1, R2, suppression) |
| Écran : *Traces importées* dans l'inspecteur d'un membre (fichier, aperçu, fuseau proposé = celui du navigateur, rejets motivés, empreinte, retrait confirmé) ; après import, la frise se cale sur la trace et les détecteurs tournent | `workspace/imports.rs` | code ; vu sur serveur de test avec un GPX de mars 2026 (661 points, 1 rejet, visite de zone, arrêt, pertes de signal) |

Écarts assumés : le fichier d'origine n'est pas conservé (seule son
empreinte) ; dessiner un cercle à la souris sur la carte reste à faire
(saisie par formulaire) ; une zone plus fine que le pas entre deux points
peut être traversée sans être vue ; `map_pin` (POI) n'a pas encore de zone
(seul `place` en porte une).

### 2026-10-12 (suite) — seuils par dossier (fin de L5), gels, export Markdown et réimport (L6) (non commité)

| Élément | Où | État |
|---|---|---|
| Seuils des détecteurs = propriétés de `case` (`copresence_distance_m`, `copresence_min_s`, `stop_diameter_m`, `stop_min_s`, `signal_gap_s`, bornées par le schéma) ; défauts dans `pnex-core` ; la lecture des traces prend le trou de signal du dossier ; seuils effectifs cités dans `params` de chaque proposition | `ontology/mod.rs`, `track/detect.rs`, `cases/detect.rs` (`Thresholds`), `cases/tracks.rs` | code + test d'intégration (bornes refusées, seuil repris dans `params`) |
| Cadre : section *Seuils des détecteurs* (vide = défaut affiché en gris) ; l'enregistrement relance le calcul une fois la sauvegarde faite | `workspace/frame.rs`, `workspace/mod.rs` (`save_object_then`) | code ; vu en clair, sombre, 390 px |
| Gels : `POST/GET …/snapshots`, `GET …/snapshots/{seq}` ; contenu = export `pnex.case/1` + propositions examinées + traces généralisées autour des événements (± 1 h, ≈ 1 point par pixel sur 1 000 px, omises au-delà de 2 Mo ou de 31 jours) ; JSON canonique (clés triées, compact), SHA-256, numéro croissant sous verrou de la ligne du dossier ; aucune route ne réécrit un gel (405) | `cases/snapshots.rs`, `api.rs` (`CaseSnapshotContent::canonical`) | code + test d'intégration (numéros, empreinte recalculée, 405, 404, R1, R2) |
| **Écart au §7** : `case_snapshots.content` est `text`, pas `jsonb` — `jsonb` réordonne les clés et normalise les nombres, l'empreinte ne serait plus vérifiable sur les octets relus | migration 000013 | schéma |
| Purge (K16) : purger une personne supprime les gels qui la citent | `cases/purge.rs` (`snapshots::forget`) | code + test |
| Export Markdown (courant ou gel) rendu côté front dans la langue du lecteur : cadre, chronologie avec attaches et rôles, non daté, hypothèses, pièces (tableau), et pour un gel : numéro, empreinte, détections retenues ou écartées | `workspace/markdown.rs` (+ test), `workspace/snapshots.rs`, bouton *Markdown* de l'en-tête | code + test + e2e |
| Réimport : `POST /cases/import` (export ou gel) → nouveau dossier ouvert de la même org ; événements recréés, attaches vers les objets encore présents (rôle, précision), pièces non datées ; les objets absents ou d'une autre org sont listés, pas recréés (R1) ; bouton *Importer* de la liste | `cases/export.rs` (`import`), `pages/cases/mod.rs` | code + test d'aller-retour (export → import → export identique hors identifiants) + e2e |
| Assistant : fiche mise à jour (seuils, gels, Markdown, import) ; aucun outil d'écriture (il ne gèle rien, D123) | `assistant-kb/cases.md` | fiche |
| e2e : seuil enregistré, gel, téléchargement Markdown, réimport de l'export ; installation des packs dans `beforeEach` avec reprise (course entre workers sur base neuve) | `e2e/tests/cases.spec.ts` | 12/12 (fr + en) |

Écarts assumés : JSON canonique « maison » (clés triées en UTF-8, nombres au
format serde_json), pas la forme RFC 8785 des nombres ; le code d'erreur
`case-snapshot-immutable` n'existe pas (aucune route d'écriture, 405) ;
le réimport ne vise que la même org (un import inter-org devrait recréer
les objets) ; l'aide clavier de L6 reste à faire.

### 2026-10-12 (suite) — frise longue durée, aide clavier (fin de L6), journal K38, graphe L4 (non commité)

Demande de Shan : bornes saisissables, ajustement sur toute l'enquête
« même si elle a duré 20 ans », et une façon de compacter.

| Élément | Où | État |
|---|---|---|
| Bornes de la fenêtre saisies (début / fin, en UTC ou heure locale selon le choix) | `workspace/timeline.rs` (`Toolbar`), `model.rs` (`input_value`, `parse_input`) | code + tests |
| Zoom jusqu'à deux siècles ; graduations calendaires au-delà d'un mois (1ᵉʳ du mois, 1ᵉʳ janvier ; l'ancien pas de 365 j dérivait d'une quinzaine de jours sur 50 ans) ; pas de 5, 10, 25, 100 ans | `model.rs` (`tick_step`, `ticks`, `tick_label`) | code + tests |
| **Axe compacté** (actif par défaut, bouton *Compacter*) : échelle linéaire par morceaux ; une période vide entre deux événements de plus de 8 % de la fenêtre, et les bords vides au-delà de la marge, se replient en une coupure d'au moins 56 px (8 % au plus sur téléphone) étiquetée « ≈ 6 ans », dates au survol ; graduations par tronçon ; gestes (clic, glisser, déplacer) passent par l'échelle | `model.rs` (`Scale`, `Stretch`), `timeline.rs` (`BreakMark`) | code + tests ; vu sur un dossier 2005 → 2025 |
| **Débordement** : au-delà de 6 lignes d'étiquettes (4 pour les signaux), le reste se regroupe en pastilles « +N » ; un clic cadre la fenêtre sur ce qu'elles contiennent | `model.rs` (`overflow`), `timeline.rs` (`MoreChip`) | code + test ; vu (40 événements en une semaine) |
| Événements hors fenêtre non dessinés (ils débordaient sur la colonne des libellés) | `timeline.rs` | code |
| Fenêtre > 31 jours : pas de lecture de traces ni de détection (message « zoomez »), *Recalculer* grisé avec explication ; la limite vient de `pnex_core::track::MAX_WINDOW_DAYS`, partagée serveur / front | `mod.rs` (`Ws::too_wide`), `track/mod.rs` | code |
| Aide clavier : bouton « ? » et touche ?, `N` (événement au curseur), `R` (retenir la détection sous le curseur : un clic sur sa pastille y met le curseur) | `mod.rs` (`KeyHelp`, `on_key`) | code ; vu |
| **Journal d'accès K38** : toute lecture de traces qui contient une personne (frise, détecteurs, gel) écrit dans le stream O2 `case_reads` (qui, dossier, objets, fenêtre ; identifiants seulement) ; `GET /cases/{id}/reads` ; section *Journal d'accès aux traces* du Cadre (personnes purgées comptées, sans nom) | `cases/reads.rs`, `cases/tracks.rs` (`load` reçoit un `Reader`), `frame.rs` | code + tests (document, 200 `null` sans O2, R1) |
| **Graphe L4** : layout de forces déterministe (Fruchterman–Reingold, 300 itérations, graine = id du dossier) ; nœuds déjà placés fixes (ajout incrémental) ; nœud glissé épinglé pour la session ; *Réorganiser* | `pnex-core/src/ontology/layout.rs` | code + tests (déterminisme, voisins plus proches, incrémental, chaîne repliée, cas dégénérés) |
| Graphe biparti natif SVG : losanges (événements), cercles (objets), rôle sur l'arête au survol / à la sélection, relations d'ontologie entre membres en pointillé ; estompage au curseur (événement futur, objet dont toutes les attaches sont futures, relation pas encore ouverte), relation fermée masquée ; dessin en pixels du panneau (taille fixe des nœuds) ; plafond 300 nœuds (« N masqués ») | `workspace/graph.rs` | code + tests ; vu en clair et sombre |
| Relations entre membres (historique complet, filtrage au curseur côté client) `GET /cases/{id}/relations` ; expansion en deux temps `POST /cases/{id}/expand` (facettes comptées par type de lien et type d'objet sur la fenêtre du dossier **ou encore ouvertes**, puis voisins choisis, 100 au plus) ; *Annuler l'expansion* (pile locale), *Garder* (au bac) | `cases/graph.rs`, `graph.rs` | code + test d'intégration (une seule relation entre membres, facettes, choix, R1) ; vu |
| Disposition du graphe **enregistrée par dossier** (retour de Shan) : table `case_layouts` (migration 000013), `GET/PUT /cases/{id}/layout` (écriture R2, bornes de taille et de coordonnées) ; enregistrée au lâcher d'un nœud ou en sortant du panneau pendant un glisser, partagée par l'équipe ; un lecteur déplace sans enregistrer ; *Réorganiser* l'efface. Le cadrage ne bouge plus après un lâcher (le nœud reste sous la main), il se recale quand un nœud arrive ; la position finale est prise au relâchement | `cases/layout.rs`, `workspace/graph.rs` | code + test d'intégration + e2e (glisser, recharger, même forme ; réorganiser = automatique), 16/16 en répétition |
| Barre de la frise en deux rangées alignées (retour de Shan : débordement au hasard, heure affichée deux fois) : lecture (▶, vitesse, heure, date, fuseau ; *Recalculer*, *Compacter*) puis fenêtre (début → fin ; −, +, *Ajuster*, ?), hauteur commune des contrôles | `workspace/timeline.rs` (`Toolbar`) | code ; vu à 1 100 et 390 px |
| Installation de pack sérialisée par org (`TenantLock`, espace `ONTOLOGY_PACK`) : deux installations simultanées se percutaient (409 pour la perdante), cause de l'échec e2e intermittent sur base neuve | `services/ontology/packs.rs`, `db_lock.rs` | code + test de concurrence (échoue 2 fois sur 3 sans le verrou) |
| Barre latérale (retour de Shan) : bouton de réduction en haut à droite du logo (et sous le logo en mode réduit), déconnexion en rouge (couleur du marqueur de menu) | `pages/shell.rs` | code ; vu |

Écarts et choix : une relation enregistrée aujourd'hui sur des faits
passés commence aujourd'hui (`valid_from`) ; le graphe la montre estompée
au lieu de la masquer, et l'expansion inclut les liens encore ouverts.
Pas de regroupement par type au-delà de 300 nœuds (simple compte). Le
graphe garde la taille de son panneau (pas de zoom ni de panoramique).
La pastille « +N » cadre ce qu'elle cache, pas tout le groupe voisin.

### 2026-10-12 (suite) — refonte plein écran (retour de Shan : « très horrible… mais il y a du potentiel ») (non commité)

Maquette validée (artefact « Espace d'enquête plein écran »), puis :

| Élément | Où | État |
|---|---|---|
| L'espace de travail passe sur `EditorShell` (comme flows et diagrammes) : barre du haut (retour, titre renommable, résumé, statut, bascule **Graphe / Carte**, Cadre, exports, ?), une seule grande vue, frise en bas | `workspace/mod.rs` (`Actions`, `View`) | code |
| Bouton **+** flottant : panneau à onglets *Objets* (membres, recherche, nouvelle note) et *Non datés · N* (le bac, toujours cible de dépôt) ; plus de colonne permanente ni de bandeau « non daté » | `workspace/palette.rs` (`AddMenu`) | code |
| Inspecteur en tiroir à droite, seulement sur sélection ou via **Détections · N** (le dossier au curseur, détections à examiner) ; Échap ferme | `workspace/inspector.rs`, `timeline.rs` (`DetectionsButton`) | code |
| Frise repliable : repliée, une barre fine (lecture, heure, date, fuseau, bande avec événements et curseur cliquable, détections) ; signaux bruts masqués par défaut (bouton *Signaux*), libellés lisibles (« Créé : … ») et regroupés (« 127 objets créés ») — le serveur joint le titre de l'objet | `timeline.rs` (`MiniBar`), `model.rs` (`group_signals`, `signal_title`), `cases/signals.rs` | code + test |
| Carte : membres localisés renvoyés par `workspace` (`places` : `geo_point`, sinon centre de zone, sinon coordonnées du POI), marqueurs cliquables (sélection), liste *Localisés* (recentre et ouvre l'inspecteur), cadrage sur tout ce qui est localisé, avis « aucun fond de carte » ; corrigé : le conteneur MapLibre (forcé en `position: relative`) avait une hauteur nulle | `cases/mod.rs` (`places_of`), `workspace/map.rs` | code + test d'intégration ; vu |
| Graphe : cause de l'agglutinement trouvée — les nœuds isolés étaient repoussés très loin (boîte de 7 700 × 7 200 pour 35 nœuds) et le cadrage écrasait le cœur ; nœuds tenus dans un cadre de la forme du panneau (FR classique), espacement `√(aire / n)` donc dynamique, attraction au centre, passe finale de séparation tenant compte de la largeur des étiquettes ; zoom molette autour du pointeur, panoramique en glissant le fond, −, +, Ajuster | `pnex-core/src/ontology/layout.rs`, `workspace/graph.rs` (`Fit::view`) | code + tests (cadre, écartement minimal) ; vu |
| État vide : invitation au centre de la vue ; e2e adaptés (ouvrir **+**, onglet *Non datés*) | `mod.rs` (`EmptyCase`), `e2e/tests/cases.spec.ts` | 14/14 |

Écarts : quelques étiquettes du graphe se touchent encore sur la rangée du haut quand beaucoup de nœuds sont isolés ; la barre fine de la frise utilise un axe linéaire (pas les coupures).

Reste : journal des changements par lots (K35) ; séries à bornes absolues
et ligne de mesures ; cercle dessiné à la souris ; zone sur `map_pin` ;
L7 enquête média.
