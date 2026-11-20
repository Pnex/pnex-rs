# Fiche benchmark : MovingPandas (source des détecteurs Rust)

## 1. Identité

- Dépôt : `bench/movingpandas`, miroir GitHub `movingpandas/movingpandas`
  (les issues GitHub sont désactivées ; le suivi a migré sur Codeberg :
  `https://codeberg.org/movingpandas/movingpandas/issues`, cf. badge de `README.md`).
- Commit étudié : `b321da5` (« Add Codeberge badges »).
- Licence **confirmée : BSD-3-Clause** (`LICENSE.txt:1-3` : « Copyright (c) 2018-2026, MovingPandas developers » ;
  `pyproject.toml` : `license = "BSD-3-Clause"`). Aucun fichier `.py` ne porte d'en-tête de licence propre
  (grep `Copyright|License` sur `movingpandas/` : 0 résultat), aucun sous-dossier sous une autre licence.
  `tools/_multi_threading.py:1` cite un snippet StackOverflow (CC BY-SA) pour `split_list`, sans intérêt pour nous.
- Ce que ça permet à PNEX (MIT) : **reprise/portage du code autorisé** à condition de conserver la notice
  de copyright BSD-3 + le texte des 3 clauses (dans un `THIRD_PARTY_NOTICES` ou en tête du module Rust porté)
  et de ne pas utiliser le nom « MovingPandas » pour promouvoir PNEX. Un portage Rust ligne à ligne est une
  œuvre dérivée : on garde l'attribution. Compatible avec la règle « briques permissives obligatoires ».
- Dépendances Python (non reprises) : geopandas, shapely ≥ 2, geopy (géodésique WGS84), pyproj.

## 2. Ce que fait le projet

Bibliothèque Python d'analyse de trajectoires au-dessus de GeoPandas : un `Trajectory` = GeoDataFrame de
points indexé par instant ; une `TrajectoryCollection` = liste de trajectoires. Fournit détection d'arrêts,
découpage (splitters), généralisation (DP, TDTR…), nettoyage, clip par polygone, interpolation, métriques
(vitesse, direction, distances DTW/LCSS/Fréchet). **Ce qui nous concerne** : les algorithmes des détecteurs
(arrêts, geofence, fin de signal) et de la simplification temporelle selon le zoom, à porter en Rust.
Rien sur la co-présence (voir §5.9).

## 3. Modèle de données pertinent

- `Trajectory` (`movingpandas/trajectory.py:58`) : `id`, `obj_id` (objet mobile = notre acteur), `parent`
  (trajectoire d'origine d'un segment), `df` (GeoDataFrame, index `DatetimeIndex`), `crs`, `is_latlon`,
  `crs_units` (`trajectory.py:178-194`).
  - Construction (`trajectory.py:120-128`) : ≥ 2 lignes obligatoires (`trajectory.py:131`), tri par temps,
    **doublons d'horodatage supprimés en gardant le premier** (`df.index.duplicated(keep="first")`).
  - **Fuseau horaire** : `_handle_timezone` (`trajectory.py:159-171`) mémorise `df_orig_tz` puis fait
    `tz_localize(None)` avec un `TimeZoneWarning` : les instants deviennent naïfs (heure murale, pas UTC).
  - Validité : `is_valid` (`trajectory.py:411-423`) = ≥ 2 points et `start < end`.
- Instants seulement : pas d'incertitude temporelle, pas d'intervalle par point. Les périodes sont des objets
  ad hoc : `TRange(t_0, t_n)`, `STRange(pt_0, pt_n, t_0, t_n)` (point d'entrée/sortie + instants),
  `TRangeWithTrajId` (`spatiotemporal_utils.py:17-55`). Pas de rôle, pas de provenance, pas de versionnage.
- Segments : un segment est un `Trajectory` dont `parent` pointe la trajectoire source et dont l'id vaut
  `f"{traj.id}_{n}"` (`overlay.py:147-150`, `trajectory_splitter.py:149`).
- `TrajectoryCollection` (`trajectory_collection.py:27-96`) : `trajectories`, `min_length` (en m si CRS
  géographique, sinon unités CRS), `min_duration` ; le filtre `min_length` passe par `is_long_enough`
  (`trajectory.py:1053-1086`, comparaison **large** `accumulated >= min_length`).
- Unités : `UNITS(distance, time, time2, crs)` + `get_conversion` (`unit_utils.py:208-258`) ; si l'unité CRS
  est inconnue, on suppose le mètre avec un `MissingCRSWarning`.

## 4. Patterns UX observés

MovingPandas est une bibliothèque, pas une application : son « UX » se limite à des tracés.
- `traj.plot()` / `explore()` / `hvplot()` (`trajectory_plotter.py:56-171`) : lignes colorées par
  trajectoire ou par colonne (vitesse), marqueurs de fin (`_hvplot_end_points`, `trajectory_plotter.py:171`)
  pour montrer le sens. Aucun curseur temporel, aucune frise.
- `__str__` affiche longueur auto-formatée m/km (`trajectory.py:211-232`).
- Tutoriels notebooks : `tutorials/4-stop-detection.ipynb` (arrêts tracés en points rouges sur la trace),
  `tutorials/3-generalization-and-aggregation.ipynb` (comparaison DP vs TDTR), `9-benchmark.ipynb` (perf).
  Démo publique : `https://movingpandas.org`, galerie `https://movingpandas.github.io/movingpandas-website/`.
- Images présentes dans le dépôt : `tutorials/data/soccer_field.png`, `docs/source/_static/logo/favicon.png`.
  Rien de transposable directement à la frise PNEX ; l'intérêt est algorithmique.

## 5. Algorithmes / mécanismes réutilisables

Convention commune : quand `is_latlon` (CRS géographique, ex. EPSG:4326), les distances ponctuelles sont
**géodésiques WGS84 en mètres** (geopy `distance.distance` = Karney, `geometry_utils.py:59-85`) ; sinon
**euclidiennes en unités CRS** (`geometry_utils.py:50-56`). `measure_distance(p1, p2, geodesic, conversion)`
(`geometry_utils.py:88-99`). **Mais** plusieurs algos travaillent en coordonnées brutes (degrés !) :
DP, TDTR, MaxDistance, interpolation, clip. Signalé par algo.

### 5.1 TrajectoryStopDetector (`trajectory_stop_detector.py:125-190`)

Paramètres : `max_diameter` (m si lat/lon, sinon unités CRS), `min_duration` (timedelta). Pas de défaut.
`n_processes=1` (multiprocessing par trajectoire pour une collection, `:109-123`).

```
fn stop_time_ranges(points: [(t, p)] trié, max_d, min_dur) -> [ (t0, tn) ]
  win = []            # fenêtre courante (pts, xs, ys, ts)
  is_stopped = false; prev_stopped = false; out = []
  for (t, p) in points:
    win.push(t, p)
    if !is_stopped:                         # valeur de l'itération PRÉCÉDENTE
      k = 0
      while len(win[k..]) > 2 && win.last.t - win[k].t >= min_dur:  # >= LARGE
        k += 1
      win = win[k..]                        # plus long suffixe d'étendue < min_dur, min 2 points
    is_stopped = false
    if len(win) > 1:
      d_bbox = latlon ? geodesic((miny,minx),(maxy,maxx)) : hypot(maxx-minx, maxy-miny)
      if d_bbox < 1.5 * max_d:              # pré-filtre bon marché, STRICT
        if mrr_diagonal(win, latlon) < max_d:   # STRICT
          is_stopped = true
    if !is_stopped && prev_stopped && len(win) > 1:
      seg_begin = win[0].t ; seg_end = win[len-2].t   # dernier point encore DANS l'arrêt
      if seg_end - seg_begin >= min_dur:              # LARGE
        out.push((seg_begin, seg_end))
        win = [win.last]                              # repart du point sortant (fix #296)
    prev_stopped = is_stopped
  if is_stopped && win.last.t - win[0].t >= min_dur:  # arrêt en fin de trace
    out.push((win[0].t, win.last.t))
  return out
```

`mrr_diagonal` (`geometry_utils.py:181-196`) : 1 point → 0 ; 2 points → distance directe ; sinon rectangle
minimal orienté (shapely `minimum_rotated_rectangle`, **calculé en degrés si lat/lon**) puis distance
(géodésique si lat/lon) entre les coins 0 et 2 ; si le MRR dégénère en segment (points colinéaires), distance
entre ses extrémités.

Sémantique : un arrêt commence au premier point d'une fenêtre dont le diamètre MRR < `max_d`, s'étend tant
que tous les points y restent, finit au dernier point avant le point qui fait exploser le diamètre. Les
instants de l'arrêt sont des **points observés** (pas d'interpolation des bords). Pendant un arrêt la fenêtre
n'est plus élaguée : elle grossit sans borne.

Sorties : `get_stop_time_ranges` → `TRangeWithTrajId` ; `get_stop_segments` → segments via
`get_segment_between` (`trajectory_utils.py:6-45`) ; `get_stop_points` (`:222-270`) → point = **médiane**
de x et de y (pas le centroïde), `start_time`, `end_time`, `duration_s`, `traj_id`.

Complexité : hors arrêt, fenêtre bornée par `min_duration` (k points) → O(n·k log k) ; pendant un arrêt de
m points, chaque nouveau point recalcule min/max O(m) + MRR O(m log m) → **O(m² log m)** par arrêt long.
Parade Rust : maintenir l'enveloppe convexe incrémentale (ou un cercle englobant incrémental) et min/max
glissants ; recalcul MRR seulement si le point sort de la bbox courante.

Cas limites vérifiés par les tests : arrêt au début (`test_stop_detector_start_stop`), à la fin
(`test_stop_detector_end_stop`), deux arrêts consécutifs sans point entre eux
(`test_stop_detector_multiple_stops_without_gap`), aucun arrêt, données tz-aware
(`test_stop_detector_tzaware_data`). Trace vérifiée à la main sur `test_stop_times_middle_stop`
(y = 0,10,20,21,22,30,40 aux s = 0,1,2,4,6,8,10 ; d = 3, 2 s → arrêt [2 s, 6 s]).
Défaut connu non corrigé : l'algorithme est glouton, pas de recherche du plus long arrêt (issue Codeberg #108 :
max_d = 50 m donne un arrêt max plus court que 15 m), pas de fusion d'arrêts proches.

### 5.2 Splitters (`trajectory_splitter.py`)

Base (`:34-99`) : sur une collection, chaque sous-trajectoire est gardée si `get_length() > min_length`
(**strict**, `:74`) puis la `TrajectoryCollection(result, min_length)` re-filtre en `>=` ; les deux
comparaisons coexistent. Toutes les sous-trajectoires de < 2 points sont jetées.

- **ObservationGapSplitter** (`:177-197`), param `gap` (timedelta), `min_length=0` :
  `groupe = cumsum(t[i] - t[i-1] > gap)` (**strict**) ; chaque groupe ≥ 2 points = une trajectoire
  `"{id}_{i}"`. Les points des deux côtés du trou ne sont **pas** reliés. O(n). C'est notre détecteur
  « fin de signal » : un trou > `gap` = fin de signal à `t[i-1]`, reprise à `t[i]`.
- **TemporalSplitter** (`:119-156`), `mode="day"` (`hour→"h"`, `day→"D"`, `month→"ME"`, `year→"YE"`, ou
  toute fréquence pandas `"2h"`), `min_length=0` : `groupby(Grouper(freq))` sur l'index (heure **murale
  naïve**, cf. §3) ; le **premier point du groupe suivant est dupliqué en fin du groupe courant** (continuité
  à la frontière) ; groupes vides → warning « observation gaps ». O(n).
- **SpeedSplitter** (`:225-231`), `speed`, `duration`, `min_length=0`, `max_speed=inf` : calcule la vitesse
  (point précédent → courant, m/s si lat/lon ; la 1ʳᵉ ligne reçoit la vitesse de la 2ᵉ,
  `trajectory.py:1520-1533`), **garde les points avec `speed <= v <= max_speed`** (`between`, inclusif),
  puis `ObservationGapSplitter(gap=duration)`. Donc : coupe là où il n'y a eu aucun point « rapide » pendant
  plus que `duration`.
- **StopSplitter** (`:256-273`), `max_diameter`, `min_duration`, `min_length=0` : détecte les arrêts (5.1)
  puis segments `[start, s0.t0], [s0.tn, s1.t0], …, [sk.tn, end]` (`get_time_ranges_between_stops`,
  `:266-273`). Les points d'arrêt bornes appartiennent aux deux segments voisins.
- **AngleChangeSplitter** (`:299-346`), `min_angle=45` (degrés), `min_speed=0`, `min_length=0` :
  direction = cap géodésique initial si lat/lon (`calculate_initial_compass_bearing`,
  `geometry_utils.py:126-155`, 0° = nord, horaire) sinon azimut plan (`:158-168`) ; 1ʳᵉ direction de
  référence = celle de la 1ʳᵉ ligne (0 par construction) ; pour chaque point avec `speed >= min_speed`, si
  `angular_difference(ref, dir) >= min_angle` (**large** ; diff ramenée dans [0,180], `:171-178`) alors
  nouveau groupe et `ref = dir`. Chaque groupe (≥ 2 points) reçoit en tête le **dernier point du groupe
  précédent**. Attention : un groupe de 1 point est jeté (et son point est perdu).
- **ValueChangeSplitter** (`:368-393`), `col_name`, `min_length=0` : nouveau groupe dès que
  `v[i] != v[i-1]` ; le 1ᵉʳ point du groupe suivant est recopié en fin du groupe courant. Utile pour
  découper une trace par état (mode de transport, porte ouverte…). NaN != NaN → chaque NaN ouvre un groupe.

### 5.3 Généralisateurs (`trajectory_generalizer.py`)

- **DouglasPeuckerGeneralizer** (`:226-241`), `tolerance` **en unités CRS** (degrés en 4326 !) : shapely
  `simplify(tolerance, preserve_topology=False)` sur la LineString, puis garde chaque point dont les
  coordonnées figurent dans le résultat (test d'appartenance de coordonnées : un point répété à un sommet
  retenu est gardé aussi). O(n log n) moyen, O(n²) pire. Rust : `geo::Simplify::simplify(&eps)` (RDP)
  ou `simplify_idx` qui rend directement les indices (évite le piège d'appartenance).
- **TopDownTimeRatioGeneralizer** (`:269-313`), `tolerance` = distance **plane** shapely (unités CRS) :
  ```
  fn td_tr(pts[0..n]) :
    if n <= 2: return pts
    de = t[n-1] - t[0] (s) ; dx = x[n-1]-x[0] ; dy = y[n-1]-y[0]
    for i: r = (t[i]-t[0])/de ; d[i] = |p[i] - (p0 + r·(dx,dy))|   # position synchrone attendue
    k = argmax(d)                       # premier max
    if d[k] > tolerance:                # STRICT
      return td_tr(pts[0..=k]) ++ td_tr(pts[k..])   # k dupliqué, dédoublonné ensuite
    else return [pts[0], pts[n-1]]
  ```
  Diffère de DP : l'écart est mesuré à la **position où l'objet serait à l'instant t[i]** s'il allait à
  vitesse constante du début à la fin du sous-segment, donc un arrêt ou un ralentissement est conservé même
  sur une ligne droite (`test_tdtr_different_than_dp`). O(n log n) moyen, O(n²) pire, récursif (en Rust :
  pile explicite). **Pour la frise/carte selon le zoom** : projeter d'abord en mètres (ENU local autour du
  centroïde de la fenêtre, ou Web Mercator corrigé de cos φ), `tolerance = k · m_par_pixel(z)` avec
  `m_par_pixel = 156543.034 · cos φ / 2^z`, k ≈ 1–2 px ; précalculer pour chaque point le seuil d'élimination
  (comme une « importance » DP/TDTR) une seule fois côté serveur, puis filtrer `importance > tol(z)` côté
  client sans recalcul.
- **MinDistanceGeneralizer** (`:115-130`), `tolerance` (m si lat/lon) : garde l'indice 0 ; pour chaque point,
  si `dist(pt, dernier_gardé) >= tolerance` (**large**) on le garde ; ajoute toujours le dernier indice. O(n).
  Remarque : le point 0 est évalué contre lui-même (dist 0) donc n'est ajouté qu'une fois sauf tolerance ≤ 0.
- **MinTimeDeltaGeneralizer** (`:148-166`), `tolerance` (timedelta) : même schéma sur le temps
  (`t - t_gardé >= tol`). O(n). C'est la décimation temporelle la plus simple pour un replay ×8.
- **MaxDistanceGeneralizer** (`:184-205`), bonus : passe unique « DP glouton » en unités CRS.

### 5.4 Cleaners (`trajectory_cleaner.py`)

- **OutlierCleaner** (`:134-172`), `v_max=None`, `units=UNITS()` (m/s si lat/lon), `alpha=3` :
  ```
  if v_max is None: v_max = alpha · quantile95(vitesses point-à-point)
  prev = p[0] ; drop = []
  for cur in p[1..]:
    v = dist(prev, cur)/(t_cur - t_prev)       # géodésique si lat/lon
    if v > v_max: drop.push(cur); continue     # STRICT ; prev NON avancé
    prev = cur
  res = p \ drop ; if !is_valid(res): warn, return original
  ```
  Le pic est éliminé car on compare au dernier point **accepté**. Défaut : si le 1ᵉʳ point est lui-même
  aberrant, tout le reste est jeté (puis repli sur l'original). Division par zéro si deux instants égaux
  (impossible après dédoublonnage). O(n). Test : `test_outlier_cleaner` (trace de Porto, 100 km/h).
- **IqrCleaner** (`:78-110`), `columns={col: alpha}` : supprime les lignes hors
  `[q25 - α·IQR, q75 + α·IQR]` (strict) sur n'importe quelle colonne numérique.

### 5.5 Geofence : get_intersecting / clip / intersection (`overlay.py`)

- `TrajectoryCollection.get_intersecting(poly)` (`trajectory_collection.py:493-515`) : garde les
  trajectoires dont la LineString **intersecte** (bord compris) ; toute exception avalée (`except: pass`).
- `clip(traj, poly, pointbased=False)` (`overlay.py:219-229`) :
  - **ligne (défaut)** (`:191-216`) : segments consécutifs `(prev_pt, prev_t) → (pt, t)`
    (`trajectory.py:2097-2117` ; segment de longueur nulle décalé de 1e-8 pour éviter les intersections vides,
    `trajectory.py:1157-1167`) ; préfiltre R-tree sur la bbox du polygone (`:171-188`) ; `inter = seg ∩ poly`,
    MultiLineString éclatée (polygones à trous, #236) ; pour chaque morceau LineString :
    `t0 = prev_t + Δt · project(seg, inter.start)/len(seg)`, idem `tn` (interpolation **linéaire en
    coordonnées CRS**), arrondi µs, recalage à `prev_t`/`t` si écart < 10 ms (`is_equal`, `:75-83`) ;
    une intersection réduite à un **Point** (tangence) est ignorée (`:45-46`).
    Puis `_dissolve_ranges` (`:49-72`) fusionne les plages contiguës (`tn == t0'` ou écart < 10 ms).
    Pour chaque plage : insertion de deux lignes synthétiques aux points d'entrée/sortie, attributs copiés de
    la ligne précédente (`pad`, `:94-133`), puis `get_segment_between(t0, tn)`.
  - **points** (`:155-168`) : runs consécutifs de points qui intersectent (bord inclus) → `[t_min, t_max]`
    sans interpolation ; un run d'un seul point est perdu (`test_clip_pointbased_singlepoint_returns_empty`).
  - Collection (`trajectory_collection.py:553-581`) : garde si `get_length() >= min_length` (#374).
- `intersection(feature)` (`overlay.py:246-258`) = clip + recopie des propriétés du feature en colonnes
  `intersecting_<clé>`.
- **Pour PNEX (entrée/sortie)** : MovingPandas rend des segments, pas des événements. Dériver :
  `entrée = t0` sauf si `t0 == début de trace` (« déjà dedans »), `sortie = tn` sauf si `tn == fin`
  (« encore dedans / signal perdu dedans »). Croiser avec ObservationGap : une sortie qui coïncide avec un trou
  de signal est « inconnue », pas une sortie. Complexité O(n · coût(seg ∩ poly)), préfiltre R-tree O(log n).
  Rust : `geo::Intersects`, `geo::BooleanOps::clip(&MultiLineString, false)` (rend les morceaux à
  l'intérieur), `geo::LineLocatePoint` pour le ratio, `rstar` pour le préfiltre ; en lat/lon, projeter en ENU
  local avant (cohérent avec MovingPandas qui reste en plan).

### 5.6 Interpolation : get_position_at (`trajectory.py:826-934`)

```
fn position_at(t, method = "interpolated"):
  if t < start || t > end: Err(hors plage)            # bornes INCLUSES
  match method:
    "interpolated":  a = ligne ffill(t) ; b = ligne bfill(t)   # t exact → a = b
                     if b.t - a.t == 0 || |ab| == 0: return a.p
                     return point à (t-a.t)/(b.t-a.t) de la longueur de [a,b]   # linéaire en coords CRS
    "nearest"|"ffill"|"bfill": ligne via pandas get_indexer(method); return son point
```
`nearest` : pandas départage les égalités en prenant l'index **le plus grand** (doc pandas `get_indexer`).
Linéaire en lon/lat brut : faux à l'antiméridien (issue #274, ouverte) ; écart négligeable à l'échelle d'une
ville. Pas de version vectorisée (issue #213 ouverte : boucle Python lente). Rust : recherche dichotomique
(`partition_point`) sur les ts puis lerp → O(log n) ; pour le replay, curseur monotone O(1) amorti.
Côté client (wasm) c'est exactement ce qu'il faut pour « chargé une fois puis interpolé ». Tests :
`test_get_position_*`, `test_get_postion_at_out_of_time_range` (`tests/test_trajectory.py:191-236`).

### 5.7 Distances et CRS

- Ponctuelle : géodésique WGS84 (geopy/Karney) si lat/lon, sinon euclidienne (`geometry_utils.py:88-99`).
  `measure_distance_spherical` (haversine R = 6 371 000 m) existe mais est `@deprecated` (`:29-47`).
- Longueur : `measure_length` (`geometry_utils.py:110-123`) somme géodésique (`geopy.geodesic(*pts)`) ou
  longueur plane. Vitesse = distance/Δt × conversions (`spatiotemporal_utils.py:57-68`).
- Distances trajectoire-à-trajectoire (`distance`, `hausdorff`, `frechet`, `dtw`, `lcss`,
  `trajectory.py:1566-2015`) : **euclidiennes même en lat/lon** avec simple `UserWarning`.
- Rust : `geographiclib-rs` (Karney, même algorithme que geopy) via `geo::Geodesic.distance(a, b)`
  (geo ≥ 0.29, API métrique) ; `geo::Haversine` suffit à < 1 km (erreur ≤ 0,5 %). Pour les détecteurs en
  boucle serrée : projeter la fenêtre en ENU local (equirectangulaire : x = R·Δλ·cos φ0, y = R·Δφ) et
  travailler en mètres plans f64 ; erreur < 0,1 % sur quelques dizaines de km.

### 5.8 Crates Rust candidates (licences vérifiées sur crates.io le 2026-10-12)

| crate | version | licence | usage |
|---|---|---|---|
| `geo` | 0.33.1 | MIT OR Apache-2.0 | Simplify/simplify_idx (RDP), MinimumRotatedRect, ConvexHull, BooleanOps::clip, Intersects, LineLocatePoint, Geodesic/Haversine |
| `geo-types` | 0.7.20 | MIT OR Apache-2.0 | Point/LineString/Polygon |
| `geographiclib-rs` | 0.2.7 | MIT | géodésique Karney (= geopy) |
| `rstar` | 0.13.0 | MIT OR Apache-2.0 | R-tree : préfiltre clip, paires candidates co-présence |
| `geojson` | 1.0.0 | MIT/Apache-2.0 | geofences en GeoJSON |

Aucune n'est encore dans `Cargo.lock` de PNEX (seuls `chrono`/`chrono-tz`).

### 5.9 Co-présence : absente de MovingPandas → algorithme proposé

MovingPandas n'a ni proximité ni rencontre (grep `proximity|encounter|cpa|closest` : rien). L'issue #31
« Implement CPA » est close en renvoyant vers PostGIS `ST_ClosestPointOfApproach`/`ST_CPAWithin`
(`liblwgeom/lwlinearreferencing.c`) et `traffic/algorithms/cpa.py`. Proposition (balayage exact par paires
de segments synchronisés, généralise CPA-within à une durée minimale) :

```
fn co_presence(A, B, d_max_m, t_min, max_interp_gap) -> [Episode{t0, tn, d_min, t_dmin, quality}]
  # A, B : [(t, lon, lat)] triés ; projeter en ENU local autour du centroïde commun
  T = fusion triée des ts de A et B restreinte à [max(A.t0,B.t0), min(A.tn,B.tn)]   # O(n+m)
  runs = []; cur = None
  for [τ, τ'] consécutifs dans T:
    if gap_A(τ,τ') > max_interp_gap || gap_B(τ,τ') > max_interp_gap: close(cur); continue  # pas de preuve
    a0,a1 = posA(τ),posA(τ') ; b0,b1 = posB(τ),posB(τ')     # lerp, curseurs monotones O(1)
    r0 = a0-b0 ; v = ((a1-b1)-r0)/(τ'-τ)                     # mouvement relatif linéaire
    # |r0 + v·s|² <= d²  ⇔  (v·v)s² + 2(r0·v)s + (r0·r0 - d²) <= 0, s ∈ [0, τ'-τ]
    [s1,s2] = racines ∩ [0, τ'-τ]  (v=0 : tout ou rien)
    if non vide: étendre/ouvrir cur sur [τ+s1, τ+s2] ; d_min = min(d_min, |r0 + v·clamp(-r0·v/v·v)|)
    else close(cur)
  close(cur) ; fusionner épisodes séparés de < merge_gap ; garder durée >= t_min
```
- Exact (pas de quantification), sans hypothèse de grille ; O(n+m) par paire. Le `max_interp_gap` évite
  d'affirmer une co-présence pendant un trou de signal (on rend « inconnu » sur la frise).
- Variante grille commune (pas Δ, ex. 10 s, interpolation des deux traces) : plus simple, O(T/Δ), mais rate
  les croisements < Δ et quantifie les durées à ±Δ ; acceptable seulement pour un aperçu.
- N acteurs : préfiltre des paires par recouvrement temporel puis bbox par fenêtre de temps (seaux de
  5 min) dans un `rstar` → pire O(N²·(n+m)), typique quasi linéaire.
- Hystérésis GPS : entrer à `d_max`, sortir à `1,2·d_max` (à exposer en paramètre) ; `quality` =
  part de l'épisode couverte par des points observés vs interpolés.
- Tests à écrire : marche parallèle à 5 m constante ; croisement bref (épisode < t_min → rien) ;
  1 s vs 60 s d'échantillonnage donnent le même épisode à ± 1 s ; trou > `max_interp_gap` coupe l'épisode ;
  traces sans recouvrement temporel → vide.

### 5.10 Tests MovingPandas à porter comme cas de test Rust

- Arrêts : `movingpandas/tests/test_trajectory_stop_detector.py:37-230` (milieu, début, fin, multiples,
  sans trou, aucun, tz-aware, collection).
- Splitters : `movingpandas/tests/test_trajectory_splitter.py` — gap `:188-232`, temporel `:44-187`,
  vitesse `:233-293`, arrêts `:294-374` et `:404` (arrêt au début), angle `:430-531`, valeur `:532-590`.
- Généralisation : `movingpandas/tests/test_trajectory_generalizer.py:77-143` (DP, TDTR,
  `test_tdtr_different_than_dp`, min_time_delta, min_distance).
- Nettoyage : `movingpandas/tests/test_trajectory_cleaner.py:72-111` (OutlierCleaner, repli si invalide).
- Geofence : `movingpandas/tests/test_overlay.py:24-250` (une/deux intersections, aucun nœud dedans, points
  dupliqués, pointbased, ms, problèmes numériques, polygone vide) ;
  `movingpandas/tests/test_trajectory_collection.py:179-240` (get_intersecting, multipolygones, min_length).
- Interpolation / longueur : `movingpandas/tests/test_trajectory.py:191-295` et `:774-800`
  (`test_get_length_spherical`).
Fixture commune : `make_traj`/`Node` dans `tests/test_trajectory.py` (CRS 31256 métrique, t = 1970-01-01).

## 6. Trois patterns à reprendre

1. **Détecteurs = fonctions pures sur `&[(t, p)]` qui rendent des plages temporelles.**
   MovingPandas sépare `get_stop_time_ranges` (plages) de la matérialisation (segments, points médians).
   PNEX : crate `pnex-core::mobility` avec `fn stops(track: &[Fix], p: StopParams) -> Vec<TimeRange>` ;
   le service backend les transforme en **propositions** d'événements (instant/période + acteur + rôle
   suggéré), jamais insérées d'office. Les plages se dessinent directement comme zones sur la ligne d'acteur
   de la frise, et le gel SHA-256 stocke les paramètres + la version de l'algorithme.
2. **Découpage en couches composables (gap → nettoyage → arrêts → geofence).**
   `SpeedSplitter` = filtre + `ObservationGapSplitter` ; `StopSplitter` = détecteur + inversion de plages.
   PNEX : pipeline explicite `split_gaps` (donne aussi « fin de signal » et les trous de la frise) →
   `clean_outliers` → `stops` / `geofence_ranges` / `co_presence`, chacun testable seul, tous en mètres ENU
   locaux. Les trous de signal deviennent une barrière : aucun détecteur n'interpole à travers.
3. **TDTR pour la simplification temporelle selon le zoom.**
   Contrairement à DP il conserve arrêts et ralentissements, donc la frise et le replay restent fidèles au
   temps. PNEX : calcul serveur d'une « tolérance d'élimination » par point (variante TDTR qui rend l'erreur
   au moment du retrait), stockée avec la fenêtre de trace envoyée au client ; le client wasm filtre par
   `tol(z)` à chaque zoom MapLibre sans requête. Lecture à la volée des traces kepler `[lon, lat, alt, ts]`
   depuis O2, jamais copiées.

## 7. Trois pièges à éviter

1. **Unités mélangées degrés/mètres.** Preuve : DP, TDTR, MaxDistance utilisent shapely en unités CRS
   (`trajectory_generalizer.py:232`, `:312-313`) donc des **degrés** en EPSG:4326 ; `mrr_diagonal` calcule le
   rectangle en degrés puis mesure en géodésique (`geometry_utils.py:191-196`) ; `distance`/`lcss` lèvent un
   simple warning (`trajectory.py:1592-1597`) ; l'issue #336 montre un « Latitude must be in [-90;90] » au
   StopSplitter (ordre lat/lon). Parade : un seul type `LocalMeters` (ENU) en entrée de tous les détecteurs,
   conversion lon/lat → ENU à la frontière, paramètres toujours en mètres et secondes (`Duration`).
2. **Fuseaux horaires écrasés.** Preuve : `_handle_timezone` fait `tz_localize(None)` → heure murale naïve
   (`trajectory.py:159-171`) ; TemporalSplitter découpe donc les jours en heure locale de la source, pas en
   UTC ; issue #303 (StopDetector cassé après 0.15 sur le calcul de `duration_s`), issue #421 (horodatages
   sub-seconde : `at[start_time]` rate la ligne et ajoute une ligne fantôme). Parade : `i64` µs UTC partout
   (`chrono::DateTime<Utc>`), fuseau d'affichage seulement dans l'UI ; ne jamais indexer par égalité de
   flottants, toujours par indice.
3. **Performance quadratique et Python ligne à ligne.** Preuve : fenêtre d'arrêt non bornée + MRR recalculé
   à chaque point (`trajectory_stop_detector.py:155-169`) ; TDTR récursif avec `df.apply` (`:291-294`) ;
   `iterrows` dans OutlierCleaner/MinTimeDelta ; régression ×2,5 de `add_speed` (#477) ; pas
   d'interpolation multi-instants (#213) ; algo glouton sensible aux paramètres (#108). Parade : Rust O(n)
   ou O(n log n) avec structures incrémentales, bench sur une trace d'une journée à 1 Hz (86 400 points) en
   CI, détecteurs exécutés dans un `spawn_blocking` avec budget, et UI montrant les paramètres utilisés
   à côté de chaque proposition (l'utilisateur ajuste `max_diameter` en voyant le résultat).

## 8. Réponses aux questions ouvertes du PRD

- (a) **Traces importées CSV/GPX vs flux** : MovingPandas ne traite que des lots (DataFrame construit depuis
  n'importe quelle source, OGC Moving Features JSON via `io.py:156-215`). Ses splitters sont précisément
  l'outillage des imports : `ObservationGapSplitter` pour séparer les sessions d'un CSV, `OutlierCleaner` pour
  les sauts GPS. Recommandation : même représentation interne pour import et flux (série `(ts, lon, lat, alt)`
  dans O2), détecteurs exécutés sur une fenêtre bornée dans les deux cas ; un import n'est qu'un flux
  historique avec provenance « fichier X ».
- (b) **Graphe natif vs Cytoscape.js** : hors périmètre du projet, rien à en tirer.
- (c) **Nommage** : vocabulaire « Trajectory / segment / stop » seulement ; aucun éclairage.

## 9. Sources

Fichiers lus (commit `b321da5`) :
- `LICENSE.txt`, `pyproject.toml`, `README.md`
- `movingpandas/trajectory_stop_detector.py` (intégral), `movingpandas/trajectory_splitter.py` (intégral),
  `movingpandas/trajectory_generalizer.py` (intégral), `movingpandas/trajectory_cleaner.py` (intégral),
  `movingpandas/geometry_utils.py` (intégral), `movingpandas/overlay.py` (intégral),
  `movingpandas/spatiotemporal_utils.py`, `movingpandas/trajectory_utils.py`,
  `movingpandas/tools/_multi_threading.py`, `movingpandas/unit_utils.py:1-60, 208-258`
- `movingpandas/trajectory.py:54-210, 411-470, 826-1168, 1250-1610, 1835-1960, 2010-2131`
- `movingpandas/trajectory_collection.py:27-96, 304-581, 1106-1118`
- `movingpandas/trajectory_plotter.py` (signatures), `movingpandas/io.py` (signatures)
- Tests : `tests/test_trajectory_stop_detector.py:1-175`, `tests/test_trajectory_generalizer.py:1-110`,
  `tests/test_trajectory_cleaner.py:72-133`, `tests/test_trajectory.py:191-240`, listes des `def test_` de
  `test_trajectory_splitter.py`, `test_overlay.py`, `test_trajectory_collection.py`

URLs consultées :
- Issues Codeberg (API `https://codeberg.org/api/v1/repos/movingpandas/movingpandas/issues`) :
  #31 CPA, #108 sensibilité des arrêts, #213 interpolation multi-instants, #236 clip polygones à trous,
  #274 antiméridien, #296 point perdu après un arrêt, #303 fuseaux / StopDetector, #336 latitude hors plage,
  #374 clip min_length, #421 horodatages sub-seconde, #477 régression perf add_speed
- `https://crates.io/api/v1/crates/{geo,geo-types,geographiclib-rs,rstar,geojson}` (versions, licences)
- `https://movingpandas.org` (site officiel, cité pour les tutoriels)
