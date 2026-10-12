# Reconnaissance vocale par enrôlement consenti (Voice ID)

**Statut :** implémenté 2026-10-11 (non commité au départ). Lève le
non-objectif « identifier une personne par sa voix » (`media-ingest.md` §2)
**dans la seule forme K19 variante A** de `cases.md` : enrôlement consenti,
contrôle humain. Numéro D à attribuer à la validation (D192+ réservé).
**Renvois :** `cases.md` K19 (cadre juridique, variante B non retenue),
`media-ingest.md` D166–D167 (registre audio, process `pnex-asr`, diarisation
anonyme), `secrets.md` D112 (scellement), `security.md` R1–R20.

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

## 2. API

| Route | Rôle |
|---|---|
| `GET /api/v1/voice-id/profiles` | membres |
| `POST /api/v1/voice-id/profiles` (`VoiceProfileInput`) | admin |
| `GET/PATCH/DELETE /api/v1/voice-id/profiles/{id}` | membres / admin / admin |
| `GET /api/v1/voice-id/by-object/{object_id}` (`null` si aucun) | membres |
| `POST /api/v1/voice-id/profiles/{id}/samples?name=&content_type=` (corps = audio, 50 Mo) | admin |
| `DELETE /api/v1/voice-id/profiles/{id}/samples/{sample_id}` | admin |
| `POST /api/v1/voice-id/profiles/{id}/generate` | admin |
| `POST /api/v1/voice-id/test?name=&object_id=|model_id=` (corps = audio) | écriture |

Codes : `voice-id-*` (10, `err_codes::ALL` + `.ftl`) ; `voice-id-not-enough-speech` porte `args.found` / `args.min_windows`.

## 3. Limites connues

- Porte de parole par énergie, pas un modèle VAD : un enregistrement bruité surestime la parole (le score LOO écarte toutefois les passages de bruit).
- Seuil calibré sur l'intra-locuteur seul, pas sur des imposteurs : les bornes [0,45 ; 0,80] tiennent lieu de garde ; à mesurer sur un jeu réel (FLEURS multi-locuteurs) avant tout usage sensible.
- Pas de commutateur plateforme : la fonctionnalité est disponible dès qu'une org a un modèle d'empreinte valide (K19 demandait aussi une activation admin plateforme — non livré).
- Rotation du trousseau : `secrets::rekey` ne re-scelle pas les centroïdes ; une clé retirée rend les profils illisibles → les régénérer (l'audio est conservé).
- Génération synchrone dans la requête (≤ 60 passages, quelques secondes avec ERes2Net) ; pas de job.
