# Dashboards « Maison » — palette domotique, cartes composées, météo (D134–D141)

> **Statut : EN COURS.** Plan validé par l'utilisateur le 2026-10-04 sur
> tous les points, avec deux reports consignés au §6 (mode sombre global,
> code PIN serrure/alarme). **Lot A livré le 2026-10-04** (§4).
> Docs liés : `surfaces-controls.md` (D123–D133 : « une surface lit, un flow
> agit », contrôles d'org, format mobile/PC, provisionnement au save),
> `viz-bases.md` (D24, D31, D40, D41), `flow-engine.md` (nœuds custom),
> `security.md` (R1–R20, grille §6).

## Le constat

La palette des dashboards est pensée pour l'industriel : 11 types de widgets
en liste plate (`library.rs`), 309 symboles P&ID/SCADA en 10 catégories,
diagrammes thermo. Rien pour un usage maison : pas de carte lumière,
thermostat, volet, serrure, alarme, énergie, météo ; pas de pièces, pas
d'onglets, pas de chips d'en-tête ; les écritures se limitent à
switch/slider/button/number.

Manques déjà présents qui gênent directement :

- `thresholds` lus par gauge/stat/indicator mais **sans éditeur** (seul
  `symbol_options.rs` les édite) ;
- la vue live force `window: "1h"` (`dashboard_live.rs`) et ignore le
  preset du widget ;
- `PaletteItem::with_group()` existe mais la palette dashboard ne l'utilise
  pas.

## Ce que font les autres (étude du 2026-10-04)

Home Assistant (cartes tile/area/thermostat/alarm/energy, tile features,
badges, sections, conditions de visibilité, Mushroom, Bubble Card),
ThingsBoard (bundles control/indoor/outdoor/air quality/liquid level,
thermostat à règle), ESPHome (20 types d'entités, web_server v3), Domoticz
(switch/dimmer/selector/blinds/P1 meter, plans de pièces), openHAB
(Switch/Selection/Slider/Setpoint/Colorpicker/Rollershutter, modèle
sémantique pièce/équipement), Jeedom (widgets desktop/mobile, synthèse par
pièce), Homey, Apple Home, Google Home. Convergences retenues :

1. **Tuile** = icône + nom + état + contrôle intégré ; tap = action
   principale, appui long = fiche détail (historique, réglages complets).
2. **Couleur et icône pilotées par l'état** ; tuile grisée si donnée
   périmée ou device hors ligne.
3. **Regroupement par pièce avec agrégats** (température médiane, puissance
   sommée, « 3 lumières allumées ») et action de groupe (« tout éteindre »).
4. **Chips d'en-tête** (résumé en un coup d'œil), onglets, visibilité
   conditionnelle.
5. **Confirmation pour les écritures sensibles** (serrure, portail, alarme).
6. **État commandé ≠ état remonté** — déjà couvert par D129 (source d'état
   facultative d'un widget de contrôle).

## 0. Décisions

| # | Décision | Motif |
|---|---|---|
| D134 | **La palette des dashboards est catégorisée** (`PaletteItem::with_group`) : Maison · Capteurs · Énergie · Contrôles · Graphiques · Industriel (symboles P&ID, thermo) · Modèles. La recherche traverse les catégories. Les widgets existants sont rangés, aucun n'est retiré. | 11 types en vrac + 309 symboles industriels masquent tout usage maison. |
| D135 | **Règles d'état communes à toutes les cartes** : `WidgetOptions.states` = liste `{when (égal / ≥ / ≤), color, icon, label}` (ex. `0` → « Fermé », gris ; `1` → « Ouvert », ambre) ; carte grisée si la valeur est plus vieille que `stale_after` ou si le device est hors ligne. Éditeur de seuils pour gauge/stat/indicator ; la vue live respecte la fenêtre du widget. | Pattern commun HA/openHAB/Jeedom/TB ; corrige les deux manques existants. |
| D136 | **Bibliothèque d'icônes « maison » dessinée par PneX** (~60 icônes : ampoule, volet, radiateur, prise, porte, fenêtre, détecteurs, compteur, panneau solaire, batterie, piscine, arrosage…), même gabarit que `components/icons.rs`. Dessins originaux, jamais de conversion d'une bibliothèque tierce. | Règle « assets tiers : jamais de copie » ; cohérence visuelle. |
| D137 | **Nouvelles primitives de contrôle** (extension `ControlKind`, `ControlSpec::accepts` reste l'unique porte front + serveur) : `select` (chaîne parmi une liste fermée d'options `{value, label}`), `stepper` (− valeur + ; bornes et pas comme `number`), `command` (chaîne parmi une liste fermée de commandes, ex. `open`/`stop`/`close`), `color` (`#rrggbb` ou kelvin borné). La valeur stockée reste un `serde_json::Value` ; le nœud `control-source` émet le payload tel quel. Option transverse `confirm` (existe) étendue d'un texte de confirmation. | Sans liste de choix ni commande, ni thermostat (mode), ni volet (▲■▼), ni scène, ni lumière couleur. |
| D138 | **Cartes composées = N contrôles nommés par rôle + sources d'état nommées.** `WidgetOptions.home = {card, controls: {rôle → ControlRef}, sources: {rôle → SourceRef}, …}` ; le save provisionne un contrôle par rôle manquant (extension D131, clé `dash-xxxxxxxx.w-0001.setpoint`, libellé « Titre · Consigne ») ; le nœud `control-source` les liste sous la carte (D133). Un widget composé reste un widget : même grille, même versioning (D24), mêmes droits. Catalogue initial (§2). | Une carte thermostat = consigne + mode + température mesurée ; un flow par rôle reste lisible. |
| D139 | **Mise en page mobile façon application** : `layout.pages = [{id, title, icon, sections}]` (1 page = comportement actuel, désérialisation `serde(default)` des dashboards existants) ; une section peut être une **pièce** (icône + agrégats optionnels sur ses cartes : moyenne, somme, compte « allumés » + action de groupe) ; barre de **chips** en tête de page ; **fiche détail** au tap long (sparkline O2 + réglages complets) ; **visibilité conditionnelle** par carte (état / seuil) ; grille 2 colonnes mobile, 3–4 colonnes en écran large. | Le même dashboard « mobile » doit être bon sur téléphone, tablette murale et PC. |
| D140 | **Nœud `pnex-weather`** (source temporisée, sans entrée) : config `{provider, lat, lon \| poi_id, interval (≥ 10 min), units}` ; trois sorties : **actuel** (température, ressentie, humidité, vent, rafales, direction, pression, précipitations, couverture nuageuse, code de condition normalisé, jour/nuit), **journalier** (7 jours : min/max, précipitations et probabilité, vent max, UV, lever/coucher), **horaire** (48 h). Chaque msg est un objet plat stable (`payload`), `topic` = `weather.current|daily|hourly`. L'utilisateur le câble à `memory-write` (live Valkey) ou `pnex-metric` (séries O2), puis le réutilise partout (dashboards, annotations, flows). Fournisseurs en liste blanche côté serveur (URL jamais saisie par l'utilisateur, R8) ; premier fournisseur à arbitrer à l'implémentation selon la licence d'usage (Open-Meteo : gratuit non commercial ; MET Norway : CC BY 4.0, User-Agent obligatoire). Carte **Météo** (actuel + prévisions) qui lit ces clés mémoire. | Choix utilisateur : la météo est une donnée d'org réutilisable, pas un widget autonome qui appelle une API. |
| D141 | **Accélérateurs** : « Depuis un device » propose la carte composée selon le pin / la métrique (température → thermo-hygro, `digital_out` → lumière ou prise, `pwm_out` → variateur) ; modèles de dashboards **Maison, Énergie, Sécurité, Jardin/Piscine** ; « Créer le flow » pré-câblé par carte composée (`control-source` → `device-write` par rôle). | Raccourcir le chemin « je branche un relais » → « j'ai une tuile lumière qui marche ». |

## 1. Lots d'implémentation

Ordre : **A → B → C → D → E**, un commit par lot, revue sécurité §6 avant
chaque commit, clés i18n FR + EN dans le même commit.

| Lot | Contenu | Décisions |
|---|---|---|
| A — Socle | Palette catégorisée, règles d'état + périmé, éditeur de seuils, fenêtre live, icônes maison | D134–D136 |
| B — Primitives | `select`, `stepper`, `command`, `color` (core + backend + cartes surface + page Contrôles + nœud) ; tests viewer → 403 conservés | D137 |
| C — Cartes composées | Modèle `home`, provisionnement multi-rôles, cartes par domaine (§2), d'abord éclairage / climat / ouvrants / capteurs, puis énergie / sécurité / appareils | D138 |
| C' — Météo | Nœud `pnex-weather` + carte Météo | D140 |
| D — Mobile | Pages, pièces + agrégats, chips, fiche détail, visibilité, colonnes adaptatives | D139 |
| E — Accélérateurs | Depuis un device, modèles, flows pré-câblés | D141 |

## 2. Catalogue des cartes (lot C)

| Domaine | Cartes | Rôles (contrôles / sources) |
|---|---|---|
| Éclairage | Lumière, Groupe de lumières | `power` switch, `level` slider, `color` color / `state` |
| Climat | Thermostat, Ventilateur, Déshumidificateur, Chauffe-eau | `setpoint` stepper, `mode` select, `preset` select, `speed` select / `current`, `humidity` |
| Ouvrants | Volet / store, Portail / garage (confirmation), Vanne / arrosage | `command` command, `position` slider, `run_minutes` number / `position`, `state` |
| Sécurité | Serrure (confirmation), Alarme (modes), Capteur binaire (porte, fenêtre, mouvement, fumée, fuite, CO), Caméra (CameraHub D73+) | `lock` command, `arm` select / `state` |
| Énergie | Puissance live, Compteur par période (kWh / eau / gaz, jour-semaine-mois), Flux énergétique (réseau ↔ solaire ↔ batterie ↔ maison), Autoconsommation | sources seules ; le compteur par période demande un mode d'agrégation par delta dans `series-batch` (O2) |
| Environnement | Thermo-hygro, Qualité de l'air (CO₂, COV, PM avec zones), Luminosité / bruit, Météo (D140) | sources seules |
| Scènes | Bouton de scène, Bande de scènes | `scene` button ou select |
| Appareils | Statut + progression (lave-linge, etc.), Batteries / signal, Devices hors ligne | sources ; les deux derniers lisent la présence (D108) et les métriques `battery` / `rssi` |
| Infos | Horloge / date, Tuile générique « entité » | — |

## 3. Sécurité (grille §6 à chaque lot)

- R1/R2 : écritures de contrôles inchangées (org du principal, garde de
  rôle, viewer → 403) ; chaque nouvelle primitive a son test viewer.
- `accepts()` reste l'unique validation ; listes `select` / `command`
  fermées, bornées en taille (options ≤ 32, valeurs `[A-Za-z0-9_.-]{1,64}`,
  libellés ≤ 64) ; `color` validé au motif.
- Règles d'état et libellés = texte, jamais de HTML (R11).
- Météo : fournisseurs en liste blanche serveur (R8), aucun secret dans
  flows.json, intervalle plancher pour ne pas se faire bannir.

## 4. Journal d'implémentation

- **Lot A (2026-10-04)** — palette en groupes Démarrer / Commandes /
  Valeurs & états / Graphiques / Industriel (`library.rs`,
  `PALETTE_GROUPS`, test : chaque type dans un seul groupe) ;
  `WidgetOptions.states` / `icon` / `stale_after_s` + `resolve_state`,
  `is_stale`, `validate_states` (`viz.rs`) ; panneau Apparence de
  l'inspecteur (`dashboard_editor/appearance.rs` : icône, seuils
  gauge/stat/indicator, règles d'état stat/indicator, délai de
  péremption) ; rendu stat/indicator avec règles + icône, carte grisée si
  périmée ; vue live : chaque série lue sur la plus large fenêtre demandée
  par ses widgets (`series_specs`) ; 99 icônes maison originales en 10
  catégories (`components/home_icons/`, noms `hicon-*` FR/EN, picker
  recherchable). Écart : « device hors ligne » ne grise pas encore la
  carte (seule la péremption de la valeur le fait) — branché avec la
  présence D108 au lot C.

## 6. Reports consignés

| # | Sujet | Statut |
|---|---|---|
| REP-1 | **Mode sombre global.** Aucune classe `dark:` ni thème dans l'UI (Tailwind v4, gris codés en dur, couleurs de widgets littérales dans `dashboard_widget.rs`). Très attendu sur un dashboard domotique mobile, mais demande de repasser sur toutes les pages : chantier transverse séparé (jetons de couleur, variante sombre des symboles et des cartes). | Reporté (décision utilisateur 2026-10-04) |
| REP-2 | **Code PIN serrure / alarme.** Pratique courante (HA `code_arm_required`, clavier alarm-panel, ESPHome `alarm_control_panel` codes) : l'écriture d'un contrôle protégé exige un code, vérifié **côté serveur** (haché sur le contrôle, jamais renvoyé en lecture, limité en tentatives, journalisé). En attendant : simple confirmation (`confirm`). | Reporté (décision utilisateur 2026-10-04) |
