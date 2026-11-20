# Reconnaissance vocale par enrôlement consenti (Voice ID)

**Statut :** implémenté 2026-10-11. Lève le
non-objectif « identifier une personne par sa voix » (`media-ingest.md` §2)
**dans la seule forme K19 variante A** de `cases.md` : enrôlement consenti,
contrôle humain. Numéro D à attribuer à la validation (D192+ réservé).
**Renvois :** `cases.md` K19 (cadre juridique, variante B non retenue),
`media-ingest.md` D166–D167 (registre audio, process `pnex-asr`, diarisation
anonyme), `security.md` §9 D112 (scellement), `security.md` R1–R20.

## 1. Décisions

| # | Décision |
|---|---|
| V1 | **Domaine séparé** de la diarisation des flux : `pnex_core::voice_id` (calcul pur), `services/voice_id/` (profiles, samples, training, testing, audio, journal), `controllers/voice_id.rs`, page `/voice-id`, onglet « Voix » de l'objet Personne. Les étiquettes `S1..S16` des flux restent anonymes ; aucun profil ne les nomme (évolution possible, décision à part). |
| V2 | **Un profil par objet `person`** (type du pack Investigation), table `voice_profiles` (`object_id` unique, cascade à la suppression de l'objet). Personne archivée = pas de nouveau profil. |
| V3 | **Consentement obligatoire** à la création : date (passée), portée, preuve, case de confirmation ; `consent_recorded_by` = e-mail de l'admin. Retrait = `DELETE` du profil → profil, extraits **et leurs fichiers audio** supprimés (blobs compris). |
| V4 | **Audio conservé** (choix produit 2026-10-11, écart assumé à K19 « gabarit seul ») : chaque extrait d'enrôlement est un média `audio` de la médiathèque (`voice_samples` → `media_assets`), pour régénérer avec un autre modèle. Seul le **passage choisi** dans le lecteur de l'onglet Voix (`start` + `duration` ≤ 180 s, 2026-10-12) est conservé, en WAV 16 kHz mono décodé : jamais l'enregistrement complet. L'audio **testé** n'est jamais conservé. |
| V5 | **Génération** : extrait décodé par le ffmpeg confiné (16 kHz mono, 180 s max), découpé en passages de 8 s ; passage retenu si ≥ 3 s de parole (porte d'énergie, référence = 95ᵉ centile du RMS des trames, pas la trame la plus forte : un pic isolé masquait la parole, 2026-10-12) ; une empreinte par passage via le modèle `speaker_embedding` de l'org (process `pnex-asr`, ≤ 60 passages). Score *leave-one-out* de chaque passage ; < 0,25 = autre voix, écarté ; ≥ 3 passages retenus requis. Centroïde = moyenne normalisée ; **seuil calibré** = moyenne − 2σ des scores LOO, borné à [0,45 ; 0,80]. |
| V6 | **Gabarit scellé** : centroïde chiffré XChaCha20-Poly1305 avec le trousseau des secrets, AAD = org + id du profil (une ligne copiée ailleurs ne s'ouvre pas). Jamais dans un DTO, un log ni le journal. |
| V7 | **Test** : vérification 1:1 (`object_id`) ou identification 1:N (profils prêts d'un même modèle — deux modèles ne se comparent jamais). Sonde = moyenne des empreintes des passages ; accepté si cosinus ≥ seuil du profil ; verdict `match` seulement si le meilleur est accepté **et** devance le second accepté de ≥ 0,05, sinon `ambiguous` / `no_match`. Résultat = **proposition**, rien n'est écrit. |
| V8 | **Rôles** : création, extraits, génération, changement de modèle, retrait = owner/admin ; test = droit d'écriture ; lecture = membres. Org toujours depuis le principal. |
| V9 | **Journal** : chaque génération et chaque test → stream O2 `voice_id_usage` (ids, scores, verdict, utilisateur), best effort ; jamais de vecteur ni d'audio. |
| V10 | **Assistant** : fiche `assistant-kb/voice-id.md` ; aucun outil, il ne lit pas les profils (K19). |

## 1 bis. Reconnaissance par empreintes d'abord (D978–D985, proposé 2026-11-16)

**Pourquoi.** L'étape « identifier » (`media-review.md` D274–D278) vient
*après* la diarisation et nomme une étiquette entière (`S1`) à partir d'une
empreinte moyenne : si la diarisation a fondu plusieurs voix dans `S1`, elles
reçoivent toutes le même nom. Or la diarisation par fenêtres ne sépare pas au
delà de 3 voix (mesure AMI du 2026-11-16 : 28,8 % de DER au mieux ; un débat
radio à ~6 voix rendu en 2 ou 3, `media-ingest.md` lot 5). En différé on a
l'enregistrement entier et le temps : on reconnaît d'abord les personnes
connues, morceau par morceau, et on ne regroupe que le reste.

| # | Décision |
|---|---|
| D978 | **Ordre inversé, en différé seulement** (fichiers et enregistrements stockés ; le direct garde la diarisation anonyme de D166) : 1) découpage fin de la parole en **morceaux de 1,5 à 3 s** (tours de la segmentation recoupés, sans faire confiance aux étiquettes `S*`) ; 2) une empreinte par morceau (modèle `speaker_embedding` de l'org) ; 3) comparaison à **toutes** les empreintes prêtes de l'org (même modèle, V7) ; 4) lissage temporel ; 5) regroupement des morceaux non reconnus ; 6) propositions d'attribution (D277), jamais d'écriture d'office. |
| D979 | **Nombre de personnes illimité** : aucune borne au nombre de profils comparés ni au nombre d'inconnus ; le coût est linéaire en morceaux × profils (calcul vectoriel, négligeable devant l'extraction d'empreintes). |
| D980 | **Décision par morceau** : cosinus avec chaque centroïde ≥ seuil calibré du profil (V5) **et** écart ≥ 0,05 avec le second accepté (V7) ; sinon morceau « inconnu ». **Lissage** : un morceau inconnu ou isolé entre deux morceaux sûrs d'une même personne, à moins de 2 s, lui revient ; un tour reconnu de moins de 1 s isolé redevient inconnu. Seuils à confirmer par la mesure (D984). |
| D981 | **Inconnus regroupés sur tout le fichier** (regroupement hiérarchique, contrainte : un morceau n'est jamais fusionné avec un morceau qui le chevauche ; groupes de moins de 5 s de parole repliés sur le plus proche) → « Inconnu 1, 2… » sans limite. Étiquettes locales au média, sans lien entre médias (V1 inchangé pour l'anonyme). |
| D982 | **Nommer un inconnu** dans le lecteur = proposer ses passages comme extraits du profil de la personne (route `samples/from-media` existante, V4 : passage choisi, ≤ 180 s) ; la régénération du profil reste un geste admin avec consentement déjà recueilli (V3, V8). Une personne sans profil ne reçoit pas de profil d'office : le nom posé reste une attribution manuelle (D277). |
| D983 | **Relancer** : sur un média, et en lot sur tous les médias de l'org (ou d'un dossier), avec les profils prêts du moment ; chaque relance remplace les propositions *non confirmées* du média, ne touche jamais aux attributions confirmées ou manuelles. Tâche de fond reprenable, journalisée (V9). |
| D985 | **API générique, pas d'usage particulier** : la reconnaissance est une étape d'analyse de média comme les autres (`identify` de `GET/POST /api/v1/media/{id}/analyses`, D274), utilisable par tout client (UI PneX, application commerciale, SDK, MCP) ; la relance en lot est une route générique sur un ensemble de médias (liste d'ids, ou un dossier), pas une route par cas d'usage. Résultat lu par les routes existantes de transcription et d'attributions. Bouton **« Régénérer la reconnaissance »** dans le lecteur du média (et sur une sélection de la médiathèque) : relance `identify` avec les profils du moment (D983). |
| D984 | **Preuve avant livraison** : mesure sur AMI (CC BY 4.0), où les réunions vont par séries de 4 avec les mêmes participants : profils entraînés sur 3 réunions d'une série, DER mesuré sur la 4ᵉ, comparé à la diarisation seule (28,8 %). Intégré seulement si le gain est net ; seuils de D980 fixés sur cette mesure. Test `crates/pnex-asr/tests/diarization_live.rs`. |

**Mesure et état (2026-11-16).** `ami_recognition` (AMI, profils entraînés
sur les autres réunions de la série, 4 participants) : DER **25,5 %** contre
27,6 % pour la diarisation seule ; **confusion de locuteur divisée par 2 à 3**
(ES2004a 13,6 → 6,8 %, TS3003a 7,4 → 2,3 %, EN2002a 16,9 → 12,0 %) ; plus de
parole manquée sur les chevauchements (deux morceaux simultanés reçoivent
parfois le même nom : amélioration suivante). Un tour de moins de 1,5 s
prend le nom reconnu pour sa voix de diarisation (sinon son voisin à ≤ 2 s) :
sans cela le DER montait à 32,2 %. **Codé** : `pnex_core::voice_recognition`
(découpage, décision, lissage, regroupement des inconnus), étape `identify`
(`services/voice_id/media.rs`) par morceaux, `propose_voices` sur des
passages, route générique `POST /api/v1/media/analyses` (lot ≤ 200 médias,
un résultat par média, `media-not-found`), bouton « Régénérer la
reconnaissance » sur la carte Analyses. **Reste** : afficher et nommer les
« Inconnu N » (D981–D982 : stockage des voix anonymes d'un média),
relance sur une sélection de la médiathèque (UI), chevauchements.

**Hors champ** : nommer les étiquettes du direct (V1) ; identifier une
personne sans profil consenti (K19 variante B) ; tout usage hors proposition
soumise à un humain.

## 2. API

| Route | Rôle |
|---|---|
| `GET /api/v1/voice-id/profiles` | membres |
| `POST /api/v1/voice-id/profiles` (`VoiceProfileInput`) | admin |
| `GET/PATCH/DELETE /api/v1/voice-id/profiles/{id}` | membres / admin / admin |
| `GET /api/v1/voice-id/by-object/{object_id}` (`null` si aucun) | membres |
| `POST /api/v1/voice-id/profiles/{id}/samples?name=&content_type=` (corps = audio, 50 Mo) | admin |
| `DELETE /api/v1/voice-id/profiles/{id}/samples/{sample_id}` | admin |
| `POST /api/v1/voice-id/profiles/{id}/samples/from-media` (`{asset_id}`) : passages confirmés de la personne dans un média (media-review.md D277) | admin |
| `POST /api/v1/voice-id/profiles/{id}/generate` | admin |
| `POST /api/v1/voice-id/test?name=&object_id=|model_id=` (corps = audio) | écriture |

Codes : `voice-id-*` (10, `err_codes::ALL` + `.ftl`) ; `voice-id-not-enough-speech` porte `args.found` / `args.min_windows`.

Auto-enrôlement depuis l'enregistreur PNeX (2026-10-30, `call-recording.md` R1.2) : mêmes routes, la personne consent pour elle-même dans la fenêtre ; preuve `PNeX recorder: self-enrolment of <e-mail>`.

## 3. Limites connues

- Porte de parole par énergie, pas un modèle VAD : un enregistrement bruité surestime la parole (le score LOO écarte toutefois les passages de bruit).
- Seuil calibré sur l'intra-locuteur seul, pas sur des imposteurs : les bornes [0,45 ; 0,80] tiennent lieu de garde ; à mesurer sur un jeu réel (FLEURS multi-locuteurs) avant tout usage sensible.
- Pas de commutateur plateforme : la fonctionnalité est disponible dès qu'une org a un modèle d'empreinte valide (K19 demandait aussi une activation admin plateforme — non livré).
- Rotation du trousseau : `secrets::rekey` ne re-scelle pas les centroïdes ; une clé retirée rend les profils illisibles → les régénérer (l'audio est conservé).
- Génération synchrone dans la requête (≤ 60 passages, quelques secondes avec ERes2Net) ; pas de job.
