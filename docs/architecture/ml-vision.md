# Roadmap PNEX — Pilier ML / Vision

> **Statut :** proposé (phase de cadrage) — stack tranchée côté plateforme
> (tout-Rust, pas de Python côté plateforme), à valider par le POC étape 1.
> **Périmètre :** reconnaissance d'objets sur vidéo, machine learning
> classique, prédiction sur télémétrie IoT.
> Renvois : `roadmap.md` (axe C, phasing), `edge-model.md` (nœuds flow),
> `notifications.md` (alertes, N1–N4), `media.md` (média D21).

## Objectif

Ajouter à PNEX des capacités ML et vision **dans le workspace Rust unifié**,
sans stack Python côté plateforme. Les capacités sont exposées comme
**nœuds natifs du moteur de flows**, avec leur schéma défini dans la crate
`core`.

## Stack retenue

| Besoin | Brique | Licence | Remarque |
|---|---|---|---|
| Deep learning (inférence, entraînement léger) | **Burn** + `burn-import` | MIT / Apache-2.0 | Import de modèles ONNX pré-entraînés |
| Inférence CPU ONNX (alternative) | **tract** | MIT / Apache-2.0 | À benchmarker face à Burn sur Pi |
| ML classique (régression, clustering, classification) | **linfa** | MIT / Apache-2.0 | API proche de scikit-learn |
| Séries temporelles (prévision, anomalies, changepoints) | **augurs** (Grafana) | Apache-2.0 | Comble le manque de linfa sur le temporel |
| Décodage vidéo / RTSP | `ffmpeg-next` ou `gstreamer-rs` | bindings C | Seule dépendance native assumée |

## Nœuds du moteur de flows

- **Inférence vision** : prend une frame en entrée et produit des
  détections typées (classe, score, bbox).
- **Anomalie** : s'appuie sur augurs et s'applique à un flux de télémétrie.
- **Forecast** : s'appuie sur augurs et produit une prévision avec
  intervalle.
- **Modèle linfa** : applique un modèle entraîné et sérialisé à des
  features.

## Points d'attention

- **Décodage vidéo :** il n'y a pas de solution pure Rust mature pour
  H.264/H.265. La dépendance ffmpeg ou GStreamer doit être validée sur ARM.
- **Performance sur Raspberry Pi :** avec le backend CPU, on peut compter
  sur quelques images par seconde au mieux, et le backend wgpu est
  incertain sur le GPU du Pi. Il faut donc prévoir l'échantillonnage des
  frames, une résolution réduite, un pré-filtre par détection de
  mouvement, ou un déport vers un nœud plus puissant.
- **Couverture ONNX de burn-import :** elle est à valider modèle par
  modèle, car certains exports demandent des retouches.
- **Licence des modèles :** il faut éviter YOLOv8/v11 d'Ultralytics
  (AGPL-3.0) et privilégier **YOLOX** ou **RT-DETR** (Apache-2.0).
- **Entraînement de détecteurs en Rust :** c'est hors scope au départ,
  parce qu'il faudrait écrire la loss, l'assignation des labels et le
  calcul du mAP. On part de modèles pré-entraînés.

## Étapes proposées

1. **POC inférence :** importer un modèle YOLOX ONNX (Burn et tract),
   faire tourner la détection sur une image fixe, puis benchmarker sur Pi.
2. **Pipeline vidéo :** décoder un flux RTSP, échantillonner les frames,
   passer par le nœud d'inférence et envoyer les détections dans
   OpenObserve.
3. **Télémétrie :** créer les nœuds anomalie et forecast (augurs) sur les
   séries existantes. — **Livré 2026-09-29** (branche `feat/predictive-nodes`) :
   crate `pnex-node-predict`, nœuds `anomaly` / `forecast`, voir
   « Étape 3 : état » ci-dessous.
4. **ML classique :** créer le nœud linfa et définir le stockage et le
   versionnement des modèles entraînés.

## Étape 3 : état (2026-09-29)

- **Contrat** : `pnex_core::predictive` (configs, codes de violation,
  nombre de ports) ; kinds `anomaly` / `forecast` (config optionnelle =
  défauts utilisables tels quels depuis la palette).
- **Entrée** : une série numérique par `msg.topic` (payload scalaire, ou
  champ `key` d'un payload objet). Fenêtre glissante bornée par série
  (64 séries max par nœud), **persistée dans Valkey**
  (`pnex:series:v1:{org}:{flow}:{node}:{topic}`, TTL 30 j, best-effort
  borné à 1 s) : un redéploiement conserve l'historique.
- **Anomalie** : `robust_z` (médiane/MAD, défaut), `forecast_band`
  (AutoETS un pas, MSTL si saison), `changepoint` (BOCPD Normal-Gamma
  **maison** — la feature `changepoint` d'augurs tire argmin/peroxide/slog
  et bincode 1, alerte Dependabot sans correctif). Port 0 = détail,
  port 1 = booléen (déclencheur notify). Les ruptures déjà présentes dans
  l'historique au premier calcul sont enregistrées, jamais signalées.
- **Prévision** : `ets` (AutoETS, MSTL si saison) ou `linear` (moindres
  carrés + intervalle de prédiction — AutoETS choisit parfois « sans
  tendance » sur une dérive très lente). Pas = intervalle médian
  d'échantillonnage. Avec seuil : délai de franchissement (moyenne) et
  franchissement le plus précoce plausible (borne de l'intervalle).
  Ports : détail / booléen franchissement / secondes avant franchissement.
- **Reste** : E2E UI (palette → deploy → notification réelle) et démo
  « maintenance augmentée » (série injectée dans O2) pour pnex.io ;
  anomalie multi-séries (augurs outlier : « quelle pompe sur 10 dérive »)
  en v2.

## Questions ouvertes

- Où tourne l'inférence vidéo : sur le serveur central, sur un nœud edge
  dédié, ou sur les deux selon la charge ?
- Où stocker et versionner les modèles (**RustFS** — le S3-compatible de
  référence du projet — ou Postgres) ? L'école
  `media_assets` (média D21) est candidate pour le registre de modèles.
- Quel est le besoin réel de fine-tuning sur des données client ? (le
  « Model Lab » de la roadmap — fabrication de modèles — démarre par le
  ML classique linfa, entraînable en Rust ; le fine-tuning de détecteurs
  est une décision explicite ultérieure — proposée par
  `media-vision-studio.md`, Vision Lab, 2026-10-10)
