# Cas d'usage PNeX — inventaire

> Statut : **inventaire de travail** (2026-09-29). Sert de matière à la page
> d'accueil de pnex.io (section « Cas d'usage » + vedette « Maintenance
> augmentée ») et de liste de démos à capturer.
>
> Légende statut :
> - ✅ **faisable aujourd'hui** avec les briques livrées
> - 🔶 **avec les nœuds anomalie / forecast** (tranche P2.3 étape 3, en cours)
> - 🔭 **futur** (dépend d'un item roadmap non livré — cité)
>
> Règle site : on ne publie en vitrine que ✅ (et 🔶 une fois livré + capturé).

Briques citées : **Flows** (éditeur visuel, calc, fonctions JS/Starlark),
**Devices** (générique ESP8266/ESP32/C3, pinout, read/write/PWM, OTA),
**Télémétrie** (O2), **Alertes** (notify N1–N4 : ntfy, mail, webhook…),
**SCADA** (dashboards), **Carte/POI**, **Visite 360°** (take360 + Studio +
annotations), **Caméra/vision** (ESP32-CAM, YOLOX, journal d'événements),
**Assistant IA** (lit la télémétrie, rédige flows/fonctions, lecture seule
sur les devices), **Thermo** (CoolProp), **Régulation** (cartes castées D20).

---

## 1. Économie d'énergie

| # | Cas | Capteurs / actionneurs | Briques | Statut |
|---|---|---|---|---|
| E1 | **Suivi de consommation électrique** par circuit / machine (kWh, puissance, talon nocturne) | pince ampèremétrique SCT-013, compteur à impulsions, PZEM-004T, TIC Linky | Devices, Télémétrie, SCADA | ✅ |
| E2 | **Détection de talon anormal** (conso la nuit / le week-end alors que le site est fermé) | idem E1 | Flows (calc + plage horaire), Alertes | ✅ (seuil) · 🔶 (appris) |
| E3 | **Porte de chambre froide restée ouverte** → alerte après N secondes, compteur d'ouvertures, énergie perdue estimée | contact magnétique (reed), sonde T° | Devices, Flows, Alertes, SCADA | ✅ |
| E4 | **Éclairage piloté par la lumière du jour** (couper si luminosité suffisante / pièce vide) | LDR / BH1750, PIR, relais | Devices read/write, Flows | ✅ |
| E5 | **Délestage / effacement** : couper des charges non prioritaires au-dessus d'un seuil de puissance | E1 + relais | Flows, Devices write | ✅ |
| E6 | **Chauffage selon occupation** (réduit hors présence, fenêtre ouverte = chauffage coupé) | PIR, contact fenêtre, T° | Flows, Régulation | ✅ |
| E7 | **Optimisation d'installation** : comparer rendement réel vs théorique d'une PAC / groupe froid (COP, surchauffe, sous-refroidissement) | T° + pressions HP/BP | Thermo (CoolProp), Flows, SCADA | ✅ |
| E8 | **Prévision de consommation** (demain, semaine) pour lisser les pointes | E1 | Forecast | 🔶 |
| E9 | **Autoconsommation solaire** : lancer ballon / recharge quand la production dépasse la conso | production PV (onduleur Modbus/HTTP), conso | http_fetch, Flows, Devices write | ✅ |
| E10 | **Comparer des bâtiments / sites** (kWh/m², par degré-jour) | E1 + T° ext | Télémétrie, SCADA, Carte/POI | ✅ |

## 2. Maintenance (préventive, conditionnelle, prédictive)

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| M1 | **Maintenance augmentée** (vedette) : alerte → visite 360° de l'équipement annoté → question à l'IA sur les mesures | tout | Alertes, Visite 360°, Assistant IA, Flows | ✅ (hors prédictif) |
| M2 | **Dérive lente** d'une mesure (roulement qui chauffe, filtre qui s'encrasse → ΔP monte) avec **date estimée de franchissement de seuil** | T°, pression différentielle | Forecast + Alertes | 🔶 |
| M3 | **Anomalie ponctuelle** (pic, chute, valeur hors comportement habituel) sans seuil fixe à régler | toute série | Anomalie + Alertes | 🔶 |
| M4 | **Changement de régime** (rupture : la machine ne se comporte plus pareil après une intervention) | toute série | Anomalie (changepoint) | 🔶 |
| M5 | **Compteur d'heures de fonctionnement / cycles** → maintenance à l'usage (vidange toutes les 500 h) | courant, contact | Flows (calc cumul), Alertes | ✅ |
| M6 | **Courts-cycles de compresseur / pompe** (démarrages trop fréquents) | courant, contact | Flows, Alertes | ✅ |
| M7 | **Vibrations** (moteur, pompe, ventilateur) : niveau RMS qui monte | accéléromètre MPU6050 / ADXL345 | Devices, Anomalie/Forecast | ✅ (RMS seuil) · 🔶 |
| M8 | **Documentation terrain** : visite 360° du local technique avec annotations (vanne, disjoncteur, capteur) liées aux équipements | caméra 360 / téléphone | Take360, Studio, Annotations, POI | ✅ |
| M9 | **Surveillance visuelle** d'un équipement (voyant, fumée, fuite visible, présence) | ESP32-CAM | Caméra, vision-detect, journal d'événements | ✅ (détection générique) · 🔭 (modèles dédiés : Model Lab) |
| M10 | **Flotte hétérogène** : tous les capteurs d'un parc, OTA, état en ligne, carte des sites | ESP8266/ESP32 | Devices, OTA, Carte/POI | ✅ |
| M11 | **Comparer des machines identiques** (quelle pompe sur 10 se comporte différemment) | même capteur × N | Anomalie multi-séries (augurs outlier) | 🔭 (v2 du nœud) |
| M12 | **Durée de vie résiduelle (RUL)** apprise sur historique de pannes | historique + étiquettes | Model Lab (linfa) | 🔭 |

## 3. Froid, CVC, thermique

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| F1 | **Chaîne du froid** : T° chambre froide / vitrine, alerte hors plage, historique HACCP | DS18B20, SHT31 | Télémétrie, Alertes, SCADA | ✅ |
| F2 | **Dégivrage mal réglé** (T° qui remonte trop longtemps) | T° évaporateur | Flows, Anomalie | ✅ · 🔶 |
| F3 | **Diagnostic frigorifique** : surchauffe / sous-refroidissement calculés en live | pressions + T° | Thermo (CoolProp) | ✅ |
| F4 | **Régulation autonome** d'une boucle (thermostat, PID) castée sur la carte, qui continue sans serveur | T° + relais/PWM | Régulation (D20) | ✅ (serveur) · 🔭 (firmware regulator F3) |
| F5 | **Confort / qualité d'air** (CO₂, humidité, T°) → ventilation | SCD40, BME280 | Devices, Flows, Devices write | ✅ |
| F6 | **Psychrométrie** (point de rosée, risque de condensation) | T° + HR | Thermo | ✅ |

## 4. Eau et fluides

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| W1 | **Détection de fuite d'eau** au sol → alerte + coupure électrovanne | sonde de fuite, électrovanne | Devices read/write, Flows, Alertes | ✅ |
| W2 | **Fuite lente** : débit nocturne jamais nul | débitmètre YF-S201 | Flows, Anomalie | ✅ · 🔶 |
| W3 | **Niveau de cuve / citerne** + prévision de vidage | ultrason JSN-SR04T, pression | Télémétrie, Forecast | ✅ · 🔶 |
| W4 | **Pression réseau** (chute = casse, montée = vanne fermée) | capteur pression | Anomalie, Alertes | ✅ · 🔶 |
| W5 | **Pompe de relevage** : cycles, marche à sec | courant, flotteur | Flows, Alertes | ✅ |
| W6 | **Qualité d'eau** piscine / aquarium (pH, ORP, T°) | sondes pH/ORP | Télémétrie, Alertes | ✅ |

## 5. Sécurité et sûreté

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| S1 | **Détection de présence / intrusion** hors horaires | PIR, contact, ESP32-CAM | Flows, Caméra/vision, Alertes | ✅ |
| S2 | **Enregistrement vidéo sur événement** (segment AVI autour de la détection) | ESP32-CAM | camera-source, video-record, vision-detect | ✅ |
| S3 | **Travailleur isolé / local technique** : porte ouverte longtemps sans mouvement | contact + PIR | Flows, Alertes | ✅ |
| S4 | **Fumée / gaz / CO** | MQ-2, MQ-7 | Devices, Alertes | ✅ (à ne pas vendre comme dispositif certifié) |
| S5 | **Suivi GPS** d'actifs mobiles | GPS | Carte/POI | 🔭 (axe G) |

## 6. Agriculture, serre, jardin

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| A1 | **Serre** : T°/HR/lumière, ouverture d'ouvrant, brumisation | SHT31, BH1750, relais | Flows, Régulation | ✅ |
| A2 | **Arrosage selon humidité du sol** (+ météo via http_fetch) | capacitif sol, électrovanne | Flows, http_fetch | ✅ |
| A3 | **Élevage / poulailler** : porte auto au lever/coucher du soleil, T° | LDR, moteur | Flows, Devices write | ✅ |
| A4 | **Ruches** : poids, T° interne, essaimage (chute brutale de poids) | cellule de charge HX711 | Télémétrie, Anomalie | ✅ · 🔶 |
| A5 | **Gel** : alerte prévisionnelle de T° négative | T° | Forecast, Alertes | 🔶 |

## 7. Bâtiment, tertiaire, commerce

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| B1 | **Occupation de salles** (réunion fantôme, taux d'usage) | PIR, CO₂ | Télémétrie, SCADA | ✅ |
| B2 | **Carte multi-sites** de tous les équipements avec état live | tout | Carte/POI, SCADA | ✅ |
| B3 | **Visite virtuelle** d'un bien / d'un local avec points d'intérêt | 360° | Take360, Studio | ✅ |
| B4 | **Vitrines réfrigérées** d'un magasin (F1 à l'échelle d'un réseau) | T° | F1 + Carte | ✅ |

## 8. Industrie et process

| # | Cas | Capteurs | Briques | Statut |
|---|---|---|---|---|
| I1 | **Supervision SCADA légère** d'une ligne (états, compteurs, synoptique) | automates via http/Modbus-gateway, capteurs ESP | SCADA, Flows | ✅ |
| I2 | **Comptage de production / TRS** (cycles machine, arrêts) | contact, courant | Flows (calc), SCADA | ✅ |
| I3 | **Air comprimé** : fuites (conso compresseur à vide) | courant compresseur, pression | Anomalie, Thermo | ✅ · 🔶 |
| I4 | **Contrôle visuel simple** (présence pièce, comptage) | ESP32-CAM | vision-detect | ✅ (classes COCO) · 🔭 (modèles dédiés) |
| I5 | **Intégration SI** : pousser les mesures vers une base / API tierce | — | nœud SQL, http_fetch, webhook | ✅ |

## 9. Hobbyistes / makers

| # | Cas | Matériel | Briques | Statut |
|---|---|---|---|---|
| H1 | **Station météo** perso (T°, HR, pression, vent, pluie) + prévision locale | BME280, anémomètre | Télémétrie, SCADA, Forecast | ✅ · 🔶 |
| H2 | **Domotique maison** sans cloud : lumières, volets, prises, scénarios | relais, PIR, boutons | Flows, Devices | ✅ |
| H3 | **Boîte aux lettres connectée** (courrier arrivé) | reed / IR | Alertes (ntfy sur le téléphone) | ✅ |
| H4 | **Aquarium / terrarium** : T°, éclairage cyclique, pH | DS18B20, relais | Flows, Régulation | ✅ |
| H5 | **Imprimante 3D** : surveillance caméra + coupure si fumée | ESP32-CAM, MQ-2, relais | Caméra, Flows | ✅ |
| H6 | **Suivi de consommation du foyer** (Linky TIC) | module TIC | E1 à la maison | ✅ |
| H7 | **Petit écran local** (OLED/TFT) affichant les valeurs d'autres capteurs | OLED, TFT | Écran debug, Devices write | ✅ |
| H8 | **Brassage bière / fermentation** : régulation T° de cuve | DS18B20, relais | Régulation (PID) | ✅ |
| H9 | **Apprendre** : ESP flashé depuis le navigateur/desktop, flows visuels, fonctions JS — sans toolchain | toute carte ESP | Firmware générique, Flash, Flows | ✅ |
| H10 | **Plante qui a soif** (humidité sol → notification) | capteur capacitif | Alertes | ✅ |

---

## Vedette page d'accueil — « Maintenance augmentée »

1. Un capteur dérive → le nœud **forecast** prévoit le franchissement du seuil dans N jours (🔶) → **alerte**.
2. Le technicien ouvre la **visite 360°** du local : l'équipement est annoté, ses valeurs sont là.
3. Il demande à l'**assistant IA** : « depuis quand la T° du palier monte ? compare avec la charge du moteur ».
4. Il crée / l'IA rédige un **flow** de surveillance (brouillon à relire). L'IA ne pilote jamais les devices.

Démo à capturer (scénario `pnex-website/scripts/capture`) : série injectée dans O2
(roulement qui chauffe sur 3 semaines), nœud forecast + notify, visite annotée,
question à l'assistant.
