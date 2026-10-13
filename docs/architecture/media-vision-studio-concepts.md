# Media & Vision Studio — guide des concepts

> Compagnon de `media-vision-studio.md` (le PRD et ses décisions D192+).
> Ici, chaque notion est expliquée simplement, avec **où elle vit dans le
> code** et **comment l'essayer dans l'UI**. Tenu à jour à chaque lot.
> Dernière mise à jour : 2026-10-13 (branche `feat/media-vision-studio`).

## 1. La chaîne en une image

```mermaid
---
config:
  layout: elk
---
flowchart LR
  subgraph Sources
    CAM["Caméra device<br/>(ESP32-CAM…)"]
    IP["Caméra IP / flux<br/>(media_streams, D175)"]
  end
  CAM --> SRC["camera_source<br/>(une image = une référence)"]
  IP --> SRC
  SRC --> DET["vision_detect<br/>(boîtes + scores)"]
  DET --> TRK["vision_track<br/>(identifiant par objet)"]
  TRK --> ZONE["vision_zone<br/>(entrée / sortie de zone)"]
  SRC --> CAP["vision_caption<br/>(légende par le LLM de l'org)"]
  ZONE --> FUSE["media_fuse<br/>(description sourcée par fenêtre)"]
  CAP --> FUSE
  ASR["media_source<br/>(transcription)"] --> FUSE
  ASR --> DEC["text_decide<br/>(décision calibrée, Laya)"]
  ZONE --> LOG["event_log → OpenObserve<br/>(alertes, recherche)"]
  FUSE --> LOG
  DEC --> LOG
  DET -. "doutes (M4)" .-> LAB
  subgraph LAB["Vision Lab"]
    IMG["Images<br/>(photos, enregistrements, vidéos)"] --> ANN["Annotation<br/>(boîtes par classe)"]
    ANN --> VER["Version figée"]
    VER --> TRAIN["Entraînement Rust<br/>(tête YOLOX)"]
  end
  TRAIN --> REG["Modèles<br/>(registre, vérif D100)"]
  REG --> DET
```

Règle d'or (P6 / D128) : **aucun nœud vision ne commande quoi que ce
soit**. Il décrit, compte, alerte. Seul un flow déployé par un humain peut
relier une détection à `device_write`.

## 2. Les nœuds de flow

| Concept | En une phrase | Où dans le code | Essayer |
|---|---|---|---|
| **Image de caméra (frame)** | Une photo JPEG publiée sur le bus Valkey ; le flow ne transporte que sa **référence** (`frame_key`), jamais les octets (D78). | `pnex-node-camera` | Flows › nœud *Source caméra* |
| **Détection** (`vision_detect`) | Un modèle du registre trouve des objets : pour chacun un libellé, un **score** (0–1, confiance) et une **boîte** `[x, y, largeur, hauteur]`. | `pnex-node-vision/src/lib.rs`, `pnex-vision` | Flows › *Détection d'objets* |
| **Seuil** | Score minimal pour garder une détection. Plus bas = plus de trouvailles mais plus de fausses alarmes. | `min_score` du nœud | inspecteur du nœud |
| **Suivi** (`vision_track`, D192) | Relie les détections d'une image à l'autre : chaque objet reçoit un **identifiant de suivi** stable tant qu'il reste visible. Algorithme ByteTrack simplifié (deux passes : scores forts puis faibles). Événements `appear` / `disappear` avec la durée. | `pnex-node-vision/src/track.rs` | *Suivi d'objets* après la détection |
| **Zone** (`vision_zone`, D193) | Un polygone dessiné **sur l'image** (coordonnées 0–1, indépendantes de la résolution). Un objet suivi est « dedans » quand son **point d'ancrage** (bas-centre = les pieds) y est. Événements `enter` / `exit` avec `count` = occupation. `min_dwell_secs` = présence minimale avant d'annoncer l'entrée. | `pnex-node-vision/src/zone.rs` | inspecteur : clics sur la vue live |
| **Légende** (`vision_caption`, D195) | Le **LLM de l'org** (jamais un modèle de la plateforme, D119) décrit l'image, éventuellement recadrée sur l'objet détecté. Brancher le nœud = accepter d'envoyer l'image au fournisseur. | route interne `/internal/flow/ai/caption` | *Légende d'image* |
| **Fusion** (`media_fuse`, D196) | Toutes les 30 s, assemble détections, transcription et légendes et demande au LLM une description **sourcée** : une phrase sans source vérifiable est rejetée. Hiérarchie de confiance : détections = faits, parole = fiable, légendes = indicatives. | `/internal/flow/ai/fuse` | *Fusion média* |
| **Décision rapide** (`text_decide`, D197) | Un petit modèle (Laya, non génératif) répond à des questions typées (`choice`, `score`) avec des **probabilités** ; sert de filtre avant la fusion et de source d'alertes. Tourne sur le backend, en Rust (tract). | crate `pnex-decide` | *Décision* (bundle à installer) |
| **Fichier comme source** (`media_asset`, D206) | Une vidéo ou un son de la médiathèque rejoué **une fois, à vitesse normale**, comme une caméra ou une radio en direct : tous les nœuds (détection, zones, légendes, transcription) s'appliquent à un fichier téléversé. Le flux s'éteint seul à la fin. | `capture/mod.rs::media_asset_file`, `FileSource` | Données › Flux audio : type `media_asset`, puis nœud *Source caméra* (source « flux ») |
| **Plafond IA** | Légendes + fusions : 2000 appels par org et par jour (429 `flow-ai-quota`). | `settings.ai.flow_daily_calls` | — |

## 3. Le Vision Lab (créer son propre détecteur)

Page **Médias › Vision Lab** (`/lab`, `/lab/:id`).

| Concept | En une phrase | Où dans le code |
|---|---|---|
| **Projet** | Un détecteur à fabriquer : un nom et une liste de **classes** (« vanne_rouge »…). Une boîte désigne sa classe par **position** dans la liste, d'où l'interdiction de retirer une classe utilisée. | `services/lab.rs`, `pnex_core::lab` |
| **Image** | Une photo de la médiathèque ajoutée au projet (jamais copiée). | table `lab_images` |
| **Source** | Le lot d'où vient l'image (un import, un enregistrement, une vidéo). Sert au **split**. | `lab_images.source` |
| **Split entraînement / validation** | Les images de **validation** ne servent qu'à mesurer le modèle. Le split se fait **par source**, jamais image par image : deux images presque identiques d'une même vidéo ne peuvent pas tomber de chaque côté (sinon la note serait flatteuse et fausse). ≈ 20 % des sources en validation, forçable image par image. | `lab::auto_split` |
| **Boîte** | `[x, y, largeur, hauteur]` en fractions de l'image (0–1). | `LabBox` |
| **Exemple négatif** | Image cochée « aucun objet » : apprend au modèle ce qui **n'est pas** l'objet. | `negative` |
| **Position dans la source** (D203) | Pour une image tirée d'une vidéo ou d'un enregistrement, son instant en ms (`frame_ms`). Permet l'interpolation. | `lab_images.frame_ms` |
| **Import d'enregistrement** (D202) | Une image toutes les N s d'une caméra enregistrée (≤ 1 h, ≤ 200 images). | `import_recording` |
| **Import de vidéo** (D203) | Idem depuis un MP4/MOV de la médiathèque, décodé par le **ffmpeg confiné** de la capture (bac à sable, pas de réseau). Les vidéos de téléphone (index `moov` en fin de fichier) sont réparées en mémoire. Toutes les images d'une même vidéo = une seule source. | `pnex-media-capture` (`mp4.rs`, `decode_video_frames`) |
| **Interpolation** (D203) | On annote une image, on en saute quelques-unes, on annote une plus loin ; sur une image intermédiaire, « Interpoler » propose les boîtes déplacées en ligne droite. Appariement par classe, centres les plus proches. | `lab::interpolate` |
| **Pré-annotation** (D201) | Un modèle du registre propose des boîtes en pointillés ; rien n'est enregistré tant qu'on ne les garde pas. Un libellé égal à un nom de classe prend cette classe. | `services/lab.rs::suggest` |
| **Version figée** (D199) | Photo instantanée et en lecture seule des images annotées, de leurs boîtes, de leur split et de la version de chaque fichier. C'est sur elle qu'on exporte et qu'on entraîne. | `lab_dataset_versions` |
| **Export COCO** (D200) | Zip standard (images + annotations) pour entraîner ailleurs si on le souhaite. | `services/lab.rs::export` |

## 4. L'entraînement sans Python (D204)

**Principe** : on ne réentraîne pas tout le réseau. On part d'un modèle
YOLOX déjà enregistré (par exemple COCO, qui sait déjà « voir » des formes,
des textures, des objets). Son corps reste **figé** ; on ne réapprend que
sa **tête**, c'est-à-dire les dernières couches qui transforment ce que le
réseau voit en « quelle classe, où, avec quelle certitude ».

```mermaid
---
config:
  layout: elk
---
flowchart LR
  BASE["Modèle de départ<br/>(ONNX YOLOX)"] --> FEAT["tract : caractéristiques<br/>de chaque image (+ miroir)"]
  DATA["Version figée<br/>(images + boîtes)"] --> FEAT
  FEAT --> ASSIGN["Affectation<br/>(quelles cellules = quel objet)"]
  ASSIGN --> ADAM["Apprentissage des dernières couches<br/>(Adam, N époques)"]
  ADAM --> WRITE["Réécriture des poids<br/>dans l'ONNX (K classes)"]
  WRITE --> EVAL["mAP@0,5 sur la validation"]
  EVAL --> MODELS["Modèles (vérif D100)"]
```

| Concept | En une phrase |
|---|---|
| **Caractéristiques (features)** | Ce que le réseau figé « voit » en chaque point d'une grille (3 grilles : pas de 8, 16 et 32 pixels). Calculées une fois par image, puis réutilisées à chaque époque : c'est ce qui rend l'entraînement rapide sur CPU. |
| **Cellule** | Une case de grille ; chacune prédit au plus un objet. |
| **Affectation (SimOTA allégée)** | Pour chaque boîte annotée, on choisit les cellules qui doivent la prédire : celles dans la boîte, près du centre, dont la boîte prédite par le modèle de départ la recouvre le mieux. Les autres cellules apprennent « rien ici » (négatifs, échantillonnés). |
| **Époque** | Un passage complet sur toutes les cellules retenues. 30 par défaut, 300 au plus. |
| **Perte (loss)** | L'erreur que l'apprentissage fait baisser ; affichée par époque. Elle doit descendre. |
| **Adam** | La méthode d'ajustement des poids (écrite à la main, pas de bibliothèque). |
| **mAP@0,5** | La note du modèle sur les images de **validation** : 100 % = tous les objets trouvés (recouvrement ≥ 50 %) sans fausse alarme. Pas d'image de validation = pas de note. |
| **Vérification D100** | Tout modèle entrant dans le registre est chargé et exécuté une fois ; s'il échoue, il est refusé. Le modèle entraîné y passe comme un fichier importé. |
| **Exécution** | Dans le serveur, **un entraînement à la fois par serveur** (les suivants attendent). Un « battement » toutes les 10 s ; sans battement depuis 60 s (serveur redémarré), l'entraînement est marqué interrompu. Annulable. Mémoire des caractéristiques plafonnée (~256 Mo). |
| **Limite assumée** | Moins précis qu'un entraînement complet pour des objets très éloignés de ce que COCO connaît ; compenser par des images variées et plus d'époques. Pas de Python, nulle part (règle produit). |

Où : `pnex-vision/src/train.rs` (algorithme), `services/lab_train.rs`
(exécution, enregistrement du modèle), `pages/lab/training.rs` (panneau
« Entraînement »).

## 5. Boucle d'amélioration (M4, D205)

```mermaid
---
config:
  layout: elk
---
flowchart LR
  PROD["vision_detect en production"] -- "score dans la bande de doute" --> QUEUE["File « à revoir »<br/>du projet"]
  QUEUE --> HUMAN["Annotation<br/>(propositions déjà affichées)"]
  HUMAN --> VERSION["Nouvelle version figée"]
  VERSION --> TRAIN["Entraînement"]
  TRAIN --> MODEL["Modèle amélioré"]
  MODEL --> PROD
```

| Concept | En une phrase | Où / essayer |
|---|---|---|
| **Bande de doute** | Le modèle « hésite » quand un score tombe entre deux bornes (0,25 et 0,6 par défaut) : assez pour avoir vu quelque chose, pas assez pour l'affirmer. Ce sont les images qui apprennent le plus au modèle. | réglage *Images douteuses vers le Vision Lab* du nœud *Détection d'objets* |
| **File « à revoir »** (active learning) | Ces images partent dans un projet du Lab avec ce que le modèle a proposé ; au plus une par caméra toutes les 60 s, 500 en attente par projet. Une image quitte la file dès qu'elle est annotée (boîtes ou « aucun objet »). | Lab : point violet, filtre « Seulement les images à revoir » ; code `services/lab.rs::review`, route `/internal/flow/lab-review` |
| **Propagation par suivi** | Sur une vidéo : le modèle propose les boîtes de l'image, et chacune reprend la classe de la boîte la plus proche (recouvrement ≥ 30 %) de l'image annotée précédente. Utile quand le modèle ne connaît pas vos noms de classes (ex. il dit « person », votre classe est « opérateur »). | bouton *Propager* (barre des propositions) ; `lab::propagate` |
| **Guidage de diversité** | Le Lab signale ce qui manque pour qu'un modèle généralise : trop peu de sources pour une classe, objet toujours à la même taille (même distance), toujours au même endroit de l'image, toutes les images à la même luminosité. | panneau Statistiques ; `lab::stats` |
| **Luminance moyenne** | Clarté moyenne d'une image (0 = noir, 255 = blanc), mesurée à l'import ; sert au dernier avertissement. | `lab_images.luma` |
| **INT8** | Modèle quantifié (entiers 8 bits au lieu de réels 32 bits) : plus petit, potentiellement plus rapide sur Pi. **Évalué, non livré** : tract sait l'exécuter, mais un gain réel demande une calibration (projet à part), à décider après une mesure sur Pi 5 ; YOLOX-nano à 1 image/s tient déjà sur CPU. | PRD §12, état des lots |

## 6. Rapports (M5, D207–D210)

Pas de studio de rapports : **un rapport est une page**, écrite par un flow
et exportée en PDF.

```mermaid
---
config:
  layout: elk
---
flowchart LR
  CRON["inject<br/>(cron : lundi 7 h)"] --> DATA["données de la semaine<br/>(événements, descriptions, mesures)"]
  DATA --> LLM["llm_text<br/>(consigne + données → Markdown sourcé)"]
  LLM --> PAGE["page_write<br/>(page + version nommée)"]
  PAGE --> PDF["page_pdf<br/>(version → PDF dans la médiathèque)"]
  PDF --> SEND["pnex_notify<br/>(+ pièce jointe)"]
```

| Concept | En une phrase | Où dans le code |
|---|---|---|
| **`llm_text`** (D208) | Nœud générique : une consigne et des données → un texte Markdown du LLM de l'org, marqué `ai_generated`. Les données sont passées comme *données*, jamais comme ordres (protection contre l'injection). Compte dans le plafond IA de 2000 appels/jour. | `pnex-node-vision/src/llm_text.rs`, `/internal/flow/ai/text` |
| **`page_write`** (D209) | Écrit du Markdown dans une page : **une nouvelle page par message** (titre « Rapport {week} » ; `{date}`, `{time}`, `{week}` remplacés) ou **ajout en fin de page** (journal courant). Chaque écriture fige une **version nommée** marquée « écrite par un flow ». Les tableaux Markdown deviennent des paragraphes (le schéma des pages n'a pas de tableau). | `services/pages/flow_write.rs`, `seed.rs::append_markdown` |
| **Version IA** (D210) | Une version qui contient du texte venu de `llm_text` porte le badge « Texte IA · à relire ». | `doc_versions.ai_generated`, `flow_id` (migration 000019) |
| **Approbation** (D210) | Un humain lit la version dans **Pages › Versions** et clique **Approuver** ; on garde la première approbation (qui, quand). Par défaut une version IA non approuvée **ne part pas** ; décocher « Approbation requise » l'envoie quand même, avec la mention « non relu » dans le PDF. | `POST /api/v1/docs/{id}/versions/{n}/approve` |
| **`page_pdf`** (D209) | Transforme une version en PDF A4 (titre, en-tête org · version, numéros « Page n / m », date, mention IA relue / non relue), rangé dans la médiathèque. Sortie 0 **PDF prêt**, sortie 1 **À approuver** (y brancher une notification « rapport à relire »). | `flow_write::export_pdf`, `/internal/flow/page-pdf`, `pnex-node-vision/src/page.rs` |
| **« Dernière version envoyable, une fois »** | Mode de `page_pdf` pour un 2ᵉ flow (inject toutes les 15 min) : prend la dernière version approuvée (ou sans IA) pas encore envoyée par **ce** nœud. Jamais deux fois la même version. | table `doc_version_exports` (migration 000020) |
| **Bouton PDF** | Dans Pages › Versions, télécharge le PDF de n'importe quelle version, en français ou en anglais. | `GET /api/v1/docs/{id}/versions/{n}/pdf?lang=` |
| **Pièce jointe** | Le nœud *Notification* a un champ **Pièce jointe PDF** : le chemin (dans le message ou une variable du template) de l'id média produit par `page_pdf`. Seul le canal **e-mail (SMTP)** joint le fichier ; les autres envoient le message seul. Fichier illisible = e-mail non envoyé (jamais de rapport sans son PDF). | `attach_field`, `pnex-notify` (`send_with_attachments`, `attachments.rs`) |
| **Moindre privilège** | Le runtime de flows ne lit, pour joindre, **que** les PDF exportés par un `page_pdf` de la même org ; tout autre fichier de la médiathèque répond 404. | `/internal/flow/media/{id}` |
| **Déclencheur du nœud Notification** | Toujours obligatoire : pour un rapport, un *Inject* qui envoie `true` au démarrage du flow suffit, chaque nouveau PDF part ensuite. | fiche assistant `reports.md` |
| **krilla + harfrust** | krilla écrit les PDF (choisi plutôt que Typst, trop lourd) ; harfrust met en forme le texte (accents, ligatures). Les crates de mise en forme plus anciennes (rustybuzz, ttf-parser) sont désactivées : non maintenues (RUSTSEC). | crate `pnex-pdf` |
| **DejaVu Sans** | Polices embarquées dans les PDF (licence permissive, pas OFL). | `pnex-pdf/fonts/` |
| **Pourquoi pas un studio de rapports** | Tout est fait de nœuds génériques (`inject`, `llm_text`, `page_write`, `page_pdf`, `pnex_notify`) réutilisables ailleurs ; une page reste éditable à plusieurs avant l'envoi. | PRD D207 |

## 7. Ce qui reste hors de ce pilier

- Reconnaissance faciale, identification de personnes par l'image, pose
  humaine (Q1 : « poste » = poste de travail).

## 8. Relecture du soir (2026-10-13)

Commits de la branche `feat/media-vision-studio` (non poussés), à relire
dans cet ordre :

| Commit | Contenu |
|---|---|
| `7ddc25e2` | PRD : Q1–Q15 tranchées (Q9 = D128, Q10 = pas d'attente de tag) |
| `b88a2953` | D203 : vidéos dans la médiathèque et le Lab, interpolation |
| `7a3931ae` | D204 : entraînement sans Python (tête YOLOX en Rust) |
| `6c54180e` | D205 (partie) : guidage de diversité, propagation par suivi |
| `68c000c0` | D205 : file « à revoir » (active learning) + ce guide |
| `14a50fd2` | D206 : fichier de la médiathèque comme source de flux ; INT8 évalué |
| `a325b12b` | Test navigateur `e2e/tests/vision-loop.spec.ts` + correctif : un fichier téléversé sans type déclaré prend le type de son contenu reconnu (sinon le Lab refusait une PNG valide depuis SEC-30) |
| `21f3cf97` | PRD : rapports = pages écrites par des nœuds génériques (D207–D210) |
| `07640c99` | D208 : nœud `llm_text` |
| `6e99f61c` | D209/D210 : nœud `page_write`, versions IA, bouton Approuver |
| `7229d8b4`, `1022e170` | crate `pnex-pdf` (krilla + harfrust, DejaVu) |
| `e7214992` | PDF d'une version de page (bouton PDF) |
| `61b95677` | D209/D210 : nœud `page_pdf` (prêt / à approuver, « dernière envoyable ») |
| `e2a6bb46` | Pièce jointe PDF des notifications e-mail |
| `cb18d110` | PRD + ce guide à jour (rapports livrés) |
| `25cb2e60` | Test navigateur `e2e/tests/reports.spec.ts` : version IA → Approuver → PDF |

**Vérification navigateur (2026-10-13, serveur de branche :5151)** :
`vision-loop` 3/3 (import vidéo + interpolation, entraînement lancé du
panneau jusqu'au modèle enregistré, fichier comme source de flux) et
`lab` en anglais et en français : 5/5. `reports` (version écrite par un
flow, badge IA, Approuver, téléchargement du PDF) : 1/1 ; il lui faut le
jeton runtime du serveur (`PNEX_FLOW_RUNTIME_TOKEN` au démarrage, même
valeur dans `PNEX_E2E_FLOW_TOKEN`).

### Lancer la branche pour la relire

```bash
cd ../pnex-rust-wt/vision-studio
task build:frontend                                   # front release (wasm)
scripts/guarded.sh cargo build -p pnex-backend --bin pnex-server
cd crates/pnex-backend
set -a; . ../../../pnex-rust/deploy/edge/edge.env; set +a
export DATABASE_URL=postgres://pnex:pnex@localhost:5432/pnex_vision_dev
../../target/debug/pnex-server db migrate             # 000015 → 000020
PNEX_GUARD_NO_LOCK=1 ../../scripts/guarded.sh ../../target/debug/pnex-server start -p 5151 -b 127.0.0.1
# puis http://localhost:5151 (login Rauthy habituel)
```

Parcours à essayer, dans l'ordre :

1. **Médias › Bibliothèque** : téléverser une vidéo de téléphone (MP4 /
   MOV) → elle apparaît en kind « Vidéo » avec un lecteur.
2. **Médias › Vision Lab** : nouveau projet (classes), **Depuis une
   vidéo** → images toutes les N s ; annoter la 1re et la 5e, puis
   **Interpoler** sur la 3e ; choisir un modèle et **Propager** sur la 6e.
3. Panneau **Statistiques** : avertissements de diversité.
4. **Figer** une version, puis panneau **Entraînement** : modèle de départ
   (un YOLOX COCO de Modèles), 30 époques, **Entraîner** → progression,
   mAP, lien vers Modèles.
5. **Automatisation › Flows** : nœud *Détection d'objets* → section
   *Images douteuses vers le Vision Lab* → choisir le projet ; les images
   « à revoir » arrivent avec un point violet.
6. **Données › Flux audio** : type `media_asset`, choisir la vidéo → la
   rejouer comme une caméra (nœud *Source caméra*, source « flux »).
7. **Rapport** : flow *Inject* (cron) → *Texte IA* → *Écrire une page* →
   *PDF d'une page* ; sortie « À approuver » vers une *Notification*. Dans
   **Pages**, ouvrir la page → Versions : badge « Texte IA », **Approuver**,
   bouton **PDF**. 2ᵉ flow : *Inject* 15 min → *PDF d'une page* (« dernière
   envoyable ») → *Notification* e-mail avec **Pièce jointe PDF** = `asset_id`.

Tests automatiques du navigateur : `cd e2e && PNEX_E2E_BASE_URL=http://localhost:5151
PNEX_E2E_ISSUER=https://192.168.1.185/auth/v1/ PNEX_VISION_TEST_DIR=~/.cache/pnex-vision-test
bunx playwright test vision-loop lab`.

Avant de lancer le serveur de la branche : migrations `000015` à `000020`
sur `pnex_vision_dev`. Tests : `TEST_DATABASE_URL=…/pnex_test_vision`,
`PNEX_VISION_TEST_DIR=~/.cache/pnex-vision-test` pour les tests avec le
vrai YOLOX-nano.
