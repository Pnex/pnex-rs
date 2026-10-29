# Enregistrement des appels et visios — proposition

| | |
|---|---|
| Statut | **Proposition (2026-10-24)** — roadmap P2.30 ; **R1.0 + R1.1 Linux codés 2026-10-29** : enregistreur PNeX (connexion, activation, test de transcription, capture des appels, envoi et transcription) ; Windows/macOS à faire |
| Numérotation | décisions à numéroter au passage en PRD, lots **R1…R4** |
| Dépend de | ingestion média (`media-ingest.md`, P2.13), ASR (`pnex-asr`), locuteurs (`voice-id.md`), rendez-vous et compte rendu (P2.28), desktop natif |

## 1. Objectif et usages

Capter tous les appels et visios (PC et mobile), sans outil tiers de prise
de notes, et les verser dans le pilier **ingestion média** : transcription
horodatée par interlocuteur, résumé, actions, puis rapports dans PneX.

Usages visés :

- **commerciaux** : compte rendu de rendez-vous rattaché au compte et au
  contact (`meeting`, coaching par critère, P2.28) ;
- **SAV / support** : historique d'appels par client et par équipement,
  actions de suivi ;
- **réunions multi-canaux** : une même réunion vue depuis plusieurs
  sources (visio PC, téléphone, salle) regroupée en un seul événement.

Règle commune : la personne qui enregistre **annonce l'enregistrement**
en début d'appel (RGPD). Le device affiche une LED REC visible ; le
logiciel affiche une notification au démarrage avec annulation en un clic.

## 2. Briques

| Brique | Couvre | Matériel | Lot |
|---|---|---|---|
| **A. Desktop (logiciel)** | Visios sur PC, appels mobiles relayés au PC (Phone Link / Continuity) | Aucun | R1 |
| **B. Device filaire (jack)** | Tous les appels du téléphone avec un casque filaire | ESP32-C6 + ADC stéréo + SD | R2 |
| **C. Device Bluetooth HFP** | Appels GSM et visios mobiles sans fil côté téléphone | ESP32-C6 + TI CC2564C + codec + SD | R3 |
| **D. Proxy Bluetooth** | Utilisateur qui garde son casque Bluetooth | Brique C + relais HFP (puis A2DP) | R4 |

Toutes les briques alimentent le **même pipeline serveur**.

## 3. Pipeline serveur (commun)

1. **Réception** : segments audio déposés dans le stockage objet (RustFS,
   jamais MinIO) par URL présignée, PUT idempotent ou multipart reprenable.
2. **Vérification** : SHA-256 par segment ; le serveur confirme, et
   seulement ensuite le device marque le segment comme envoyé.
   Dédoublonnage par empreinte.
3. **Transcription** : `pnex-asr` (Whisper large-v3-turbo, Rust, sans
   Python), **un canal par interlocuteur** → diarisation gratuite
   « moi / l'autre » ; identification des locuteurs par `voice-id` si
   un profil existe.
4. **Enrichissement** : résumé et actions par le LLM de l'org (D119) ;
   rattachement au device, à l'appel et au contact (numéro par CLIP) dans
   l'ontologie.
5. **Restitution** : pages remplies depuis l'objet (D340), relecture média
   (`media-review.md`) — l'utilisateur ne sort pas de PneX.
6. **Alertes** : segments en attente depuis plus de X heures, SD pleine,
   batterie faible (canaux de notification existants).

## 4. R1 — Desktop (logiciel seul)

Dans l'application desktop native (Rust, sans driver virtuel).

**R1.0 — Enregistreur PNeX (codé 2026-10-29)** : crate `pnex-recorder`,
utilitaire Dioxus desktop dans la barre des tâches (Windows, Linux, macOS),
séparé de l'application desktop complète.

- Ouverture : serveur, e-mail, mot de passe (grant `password` via
  `/api/v1/oauth2/token`, contrat d'API vérifié sur `/api/v1/meta/version`).
  Session gardée (`<config>/pnex/recorder.json`, 0600, refresh token seul,
  jamais le mot de passe).
- Serveur HTTPS signé par sa propre autorité (edge local, D70) : même
  confiance au premier usage que l'application desktop — `/api/v1/meta/ca`
  lu sans vérification, empreinte SHA-256 affichée, CA épinglé dans la
  session après accord explicite.
- **Enregistrement activé** : interrupteur dans la fenêtre et coché dans le
  menu de l'icône ; il ne pilote encore aucune capture (lot R1.1).
- **Test de transcription** : 5 s de micro (`cpal`, mono 16 bits WAV) envoyées
  à `POST /api/v1/asr/models/{id}/test`, rien de conservé. Le modèle et la
  langue viennent du **profil par défaut** de l'org (`asr_profiles.is_default`,
  un par org, le premier profil créé l'est d'office, « Mettre par défaut »
  dans Flux › Modèles et profils). **Mode avancé** : choix parmi les modèles
  de transcription de l'org qui ont passé leur vérification, langue `auto`.
  Messages explicites : aucun modèle enregistré, aucun modèle vérifié, aucun
  profil par défaut.
- Modèle conseillé pour un poste léger : **Canary 180M flash int8**
  (sherpa-onnx, CC-BY-4.0, fr/en/es/de, 153 Mo en `.tar.bz2`, WER 9,6 %
  FLEURS, ≈ 18 flux temps réel CPU). Parakeet TDT 0.6B v3 int8 est meilleur
  (8,7 %), archive de 486 Mo : importable depuis que la limite de la
  médiathèque est passée à 2 Gio (`PNEX_MEDIA_MAX_BYTES`).
- Validé de bout en bout le 2026-10-29 sur la stack docker locale (edge
  HTTPS) : connexion + CA, messages « aucun modèle » / « aucun profil par
  défaut », test en mode avancé puis par défaut (extrait FLEURS français
  joué dans une source PipeWire virtuelle, transcription exacte en ≈ 335 ms).
  Pilotage par WebKitWebDriver (feature `e2e`, comme l'app desktop).

**R1.1 — Capture des appels, Linux (codé 2026-10-29)**

- **Capture** : deux flux libpulse (PipeWire via pipewire-pulse, présent sur
  toute distribution récente ; aucun outil en ligne de commande requis) —
  micro (`@DEFAULT_SOURCE@`) et moniteur de la sortie (`@DEFAULT_MONITOR@`),
  16 kHz mono chacun, même horloge serveur donc alignés sans rééchantillonnage.
  Un **WAV stéréo** sur disque, écrit au fil de l'eau : canal gauche = moi,
  droit = l'interlocuteur ; en-tête réécrit chaque seconde (fichier lisible
  après un plantage). `PNEX_RECORDER_MIC` / `PNEX_RECORDER_OUTPUT` choisissent
  d'autres sources (tests, postes à plusieurs cartes son).
- **Détection** : toutes les 2 s, liste des applis qui enregistrent (source
  outputs libpulse, binaire et nom d'appli) ; une appli d'appel (Teams, Zoom,
  Slack, Webex, Discord, Skype, Signal, WhatsApp, Jitsi, navigateurs) qui tient
  le micro lance l'enregistrement, **45 s** après qu'elle l'a relâché il
  s'arrête. Bouton « Enregistrer maintenant » pour le reste (arrêt à la main).
  Désactiver l'enregistrement arrête un enregistrement automatique en cours.
- **Consentement** : au démarrage la fenêtre remonte avec « Arrêter et
  envoyer » / « Arrêter sans envoyer » et le rappel de prévenir
  l'interlocuteur ; l'icône de la barre passe au rouge, entrée « Arrêter et
  supprimer l'enregistrement » dans son menu.
- **Envoi** : à l'arrêt, `POST /api/v1/media` (audio, nom « Appel <appli> du
  <date> ») puis analyse `transcribe` (`POST /api/v1/media/{id}/analyses`),
  qui tourne avec le **profil par défaut** de l'org (`runnable_profile`
  préfère désormais le défaut au premier par ordre alphabétique). Le fichier
  local n'est supprimé qu'une fois stocké par le serveur ; sinon il attend
  (`<data>/pnex/recordings`) et repart au lancement suivant, puis toutes les
  2 min.
- **Une seule instance** : verrou système sur `recordings/recorder.lock`
  (libéré par l'OS même après un plantage) ; deux instances capteraient le
  même appel dans le même fichier.
- Validé le 2026-10-29 sur la stack docker locale (edge HTTPS), tout en
  périphériques PipeWire virtuels : fausse appli d'appel (`parec
  --client-name=zoom`), voix « moi » et « interlocuteur » sur deux sorties
  virtuelles. Départ 2 s après la prise du micro, arrêt 47 s après le
  relâchement, fichier de 62 s exactement aligné (gauche 0–10 s, droite
  10–17 s), stocké, transcrit (`done` en 12 s).
- **Limite constatée** : le pipeline ASR transcrit la fenêtre de 30 s d'un
  seul tenant ; avec deux énoncés séparés par un silence, Canary et Parakeet
  en perdent un (instabilité du modèle : un écart d'un bit sur l'échantillon
  suffit à faire tomber la première phrase de Canary). Corrigé dans
  `pnex-asr` quand le profil a une **détection de voix** : le segment est
  coupé au milieu des silences d'au moins 2 s et chaque morceau transcrit
  seul (media-ingest.md, VAD). Un profil d'enregistrement d'appels doit donc
  avoir une détection de voix (Silero).
- Reste : Windows (WASAPI loopback) et macOS (process tap), transcription
  séparée par canal (« moi / l'autre » sans diarisation), notification
  système, envoi en flux pour les appels très longs (le WAV est lu en mémoire,
  ≈ 230 Mo par heure).

**Capture**
- Windows : WASAPI loopback (`cpal`) ; loopback par process (`wasapi`,
  Win10 2004+).
- macOS : Core Audio process taps (14.2+) ou ScreenCaptureKit (13+) — le
  desktop macOS n'est pas encore livré.
- Linux : sources « monitor » PipeWire / PulseAudio (`pipewire`).
- **Deux flux séparés** (micro + loopback), rééchantillonnés et alignés
  avec `rubato` (horloges différentes). Casque recommandé, sinon la voix
  locale repasse dans le loopback.

**Détection automatique**
- Signal principal : le micro est ouvert par une appli de visio (liste
  blanche : Teams, Zoom, Slack, Webex, navigateur).
  - Windows : `IAudioSessionManager2` ; plan B : registre
    `CapabilityAccessManager\ConsentStore\microphone`.
  - macOS : `kAudioHardwarePropertyProcessObjectList` +
    `kAudioProcessPropertyIsRunningInput`.
  - Linux : apparition d'un nœud `Stream/Input/Audio` dans PipeWire.
- Navigateur : affiner par titre de fenêtre (« Meet - … »).
- Arrêt : micro relâché, puis temporisation de 30 à 60 s.
- VAD optionnel (Silero via `ort`) pour éliminer les blancs.

**Appels mobiles via le PC**
- Phone Link (Windows, Android et iPhone) : détection via
  `PhoneExperienceHost.exe`, capture par loopback.
- Continuity (Mac + iPhone) : process `callservicesd`, process tap.
- Qualité HFP (8 ou 16 kHz), suffisante pour Whisper ; PC à portée
  obligatoire. Routage à valider sur machine.

**Hors champ** : Android et iOS bloquent la capture d'appel pour les
applis tierces. Seules passerelles : export depuis l'enregistreur natif du
téléphone, ou téléphonie SIP (Asterisk / FreeSWITCH) enregistrée côté
serveur.

## 5. R2 — Device filaire (jack), « PNEX Audio Tap »

Téléphone (jack 3,5 mm ou USB-C → jack) → carte en passthrough → casque
filaire. Interception stéréo : **sortie écouteur = interlocuteur**,
**ligne micro = utilisateur**.

- ESP32-C6 ; ADC stéréo I2S PCM1808 ou ES7243 (MCLK généré par le C6).
- 2 jacks TRRS, tap par ampli-op suiveur haute impédance, condensateurs
  de liaison sur la ligne micro (polarisation).
- Gain réglable par canal (écouteur fort, micro faible) ; microSD.
- Variante du schéma KiCad de base.
- Détection : VAD à seuil d'énergie par canal avec temporisation →
  événement `call_active`.

## 6. R3 — Device Bluetooth HFP, « PNEX Call »

**Architecture**
- ESP32-C6 : Wi-Fi, WebSocket ChaCha20, OTA, SD, I2S, écran.
- TI CC2564C (module CC2564MODA certifié) : contrôleur Bluetooth Classic
  en HCI UART, quartz 32,768 kHz.
- Hôte Bluetooth **open source et permissif** (BTstack écarté, voir §9) :
  - **retenu : pile Bluetooth de Zephyr** (Apache-2.0) : BR/EDR, HFP
    côté mains-libres (`hfp_hf`, codecs CVSD / mSBC / LC3-SWB), transport
    HCI UART H4 vers un contrôleur externe ; Zephyr tourne sur l'ESP32-C6.
    Le C6 passe donc sous Zephyr (RTOS) au lieu du bare-metal ;
  - **repli : Bluedroid d'ESP-IDF** (Apache-2.0) en mode « host only »
    sur HCI UART, avec un exemple `hfp_hf` existant ;
  - le service pack du CC2564C (script d'init TI) reste un binaire du
    fabricant chargé dans la puce, comme le firmware de tout contrôleur :
    vérifier sa licence de redistribution.
- Codec mono I2S ES8311 pour un casque filaire classique.
- Wi-Fi et Bluetooth sur deux puces → pas de coexistence à gérer.
- Prototype de validation : ESP32-A1S Audio Kit (ESP-IDF, Bluedroid,
  exemple `hfp_hf`) — jetable, pas pour le produit.

**Apports du HFP** : événements d'appel natifs (`call_state` : sonnerie,
décroché, raccroché), numéro appelant (CLIP) rattaché au contact, deux
flux séparés (SCO entrant et micro), visios mobiles couvertes.

**Stockage zéro perte**
- La SD est la source de vérité ; le Wi-Fi n'est qu'une file d'envoi.
- Segments de 10 à 30 s fermés et flushés à chaque rotation ; manifest en
  ajout par appel (segments, horodatage, CLIP, SHA-256).
- Segments chiffrés ChaCha20 sur la SD (clé du device).
- Carte pleine : arrêt et alerte, jamais d'écrasement d'un segment non
  envoyé.
- Coupure : supercondensateur 0,5–1 F + détection de perte VBUS qui ferme
  segment et manifest en moins de 100 ms (USB) ; brownout + tampon
  (batterie).

**Transmission**
- Profils Wi-Fi par priorité (bureau, maison, partage de connexion du
  téléphone), poussés depuis PneX.
- Envoi après l'appel par défaut (certains réseaux coupent les données en
  appel sans VoLTE) ; Wi-Fi coupé tant que la file est vide ; réessais à
  intervalle croissant.
- IMA-ADPCM (÷4 par rapport au WAV ; Opus trop lourd sur le device).
- Secours : vidage USB vers le desktop.

**Écran et indicateurs** : OLED SSD1306 / SH1106 allumé sur bouton
(~10 s) : Bluetooth, appel + chrono + REC, SD, file d'envoi, Wi-Fi,
batterie. LED rouge REC.

**Énergie**
- Repos : C6 en light sleep (le deep sleep perd l'état de la pile hôte),
  CC2564C en sniff.
- Réveils : eHCILL (événement Bluetooth), bouton, USB, timer RTC, SD.
- Mise en veille automatique : gestion d'énergie de Zephyr (PM) à valider sur C6.
- Interrupteurs de charge SD / OLED / codec ; buck-boost TPS63802.

**Alimentation** : d'abord USB, mais conçu et codé comme sur batterie
(même firmware). Variante batterie : 21700 5 000 mAh, chargeur BQ25895,
jauge MAX17048. Estimation à mesurer au PPK2 :

| Appels/jour | mAh/jour | 30 jours + 30 % |
|---|---|---|
| 1 h | ~80 | ~3 000 mAh |
| 2 h | ~130 | ~5 000 mAh |
| 4 h | ~230 | ~9 000 mAh |

Inconnue principale : l'intervalle de sniff imposé par le téléphone.

## 7. R4 — Proxy Bluetooth

La carte est casque côté téléphone et passerelle audio côté casque.

- **R4a, relais HFP** : décodage, enregistrement, réencodage dans les deux
  sens ; relais des commandes (décrocher, raccrocher, volume, sonnerie,
  CLIP) ; latence ajoutée ~30–60 ms. À vérifier : deux liens SCO/eSCO
  simultanés sur le CC2564C.
- **R4b, relais A2DP** (seulement si l'usage l'exige), plus AVRCP pour les boutons du casque : SBC seul, latence
  gênante en vidéo ; CPU du C6 probablement juste (piste ESP32-P4).
- Limites : AirPods et casques multipoint dégradés, casques LE Audio seuls
  incompatibles. Alternative : carte + casque filaire pour les appels
  enregistrés.

## 8. Intégration devices

| Capability | Rôle |
|---|---|
| `audio_in` | Flux audio stéréo |
| `call_active` | Événement VAD (filaire) |
| `call_state` | Sonnerie, décroché, raccroché + CLIP (Bluetooth) |
| `sd_status` | Présence, remplissage, erreurs |
| `upload_queue` | Segments et taille en attente |
| `battery` | Jauge (variante batterie) |

- Contrôle et événements : WebSocket ChaCha20 existant ; données : fichiers
  par HTTP présigné, pas par le WebSocket.
- Config poussée depuis PneX : profils Wi-Fi, politique d'envoi, rétention.
- Les flows réagissent aux événements d'appel comme à tout capteur.
- Flash depuis la plateforme, puis OTA ; catalogue en code (D121), banc
  HIL (D122).

## 9. Options écartées

| Option | Raison |
|---|---|
| Driver audio virtuel (type VB-Cable) | Pilote noyau à signer ; le loopback suffit |
| Appli mobile de capture | Bloquée par Android et iOS |
| Bluetooth sur C3/C6 seul | Pas de Bluetooth Classic, donc pas de HFP |
| ESP32 classique comme produit | Coexistence Wi-Fi/BT et RAM fragiles ; prototype seulement |
| Pi Zero 2 W + BlueZ | Exige un OS Linux |
| Pico 2 W (CYW43439) | Support SCO incertain, hors famille ESP |
| nRF5340 LE Audio | Appels LE Audio quasi absents sur iOS |
| Puces Qualcomm QCC | SDK sous NDA |
| Opus sur le device | Trop lourd ; ADPCM retenu |
| BTstack | Gratuit en non commercial seulement, licence non permissive (2026-10-24) |
| BlueZ | GPL et exige Linux |

## 10. Points bloquants avant PRD

- **Hôte Bluetooth** : BTstack écarté (licence non permissive) ; Zephyr
  retenu, Bluedroid en repli (§6). À valider : SCO/eSCO et mSBC vers le
  CC2564C via H4 (audio par HCI ou par l'interface PCM de la puce).
- **Pile firmware** : la famille actuelle est Arduino/PlatformIO (lib
  `pnex`) ; R3 impose Zephyr (pile Bluetooth), R2 peut rester sur la lib
  PIO. Le protocole PneX (WebSocket ChaCha20, OTA) est à porter sous Zephyr :
  choix de phase explicite.
- **Consentement et rétention** : annonce, rétention par org, droit
  d'effacement, accès aux enregistrements (IAM `Org("…")`), revue sécurité
  §6 ; enregistrements = données personnelles des deux parties.
- Desktop macOS non livré : la partie Continuity attend ce lot.

## 11. Points à valider

- [ ] Phone Link / Continuity capturables par loopback ou process tap
- [ ] Compatibilité HFP / mSBC avec le téléphone (prototype ESP32-A1S)
- [ ] Courant de repos réel (sniff) au PPK2
- [ ] Deux liens SCO/eSCO simultanés sur le CC2564C (R4)
- [ ] Zephyr HFP HF sur C6 + CC2564C en H4 (SCO, mSBC, service pack) et veille automatique
- [ ] Débit Wi-Fi du C6 via partage de connexion (iPhone et Android)

## 12. Ordre

1. **R1 Desktop** : double flux + détection + transcription par canal +
   pipeline ; valider Phone Link / Continuity.
2. **R2 Filaire** : prototype de la chaîne audio, puis carte « Audio Tap ».
3. **R3 Bluetooth HFP** : prototype A1S → carte C6 + CC2564C (USB, conçue
   batterie) → stockage zéro perte, envoi Wi-Fi, écran → mesures PPK2 et
   variante batterie.
4. **R4 Proxy** : relais HFP, A2DP seulement si nécessaire.
