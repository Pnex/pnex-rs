# Lot 0 — benchmark ASR (sherpa-onnx vs whisper.cpp, CPU vs GPU)

**Date :** 2026-10-09. **PRD :** `media-ingest.md` (D166, D167, §12 lot 0).
**Code :** crate `crates/pnex-asr` (trait `Transcriber`, adaptateurs
`sherpa` et `whisper`, exemple `asr_bench`).

But : ne pas refaire ce banc. Il dit ce qui tourne, à quelle vitesse,
avec quelle qualité, et comment le reproduire. **Aucun modèle n'est
imposé** : l'utilisateur importe et choisit ses modèles (registre D167) ;
ces chiffres servent à documenter et, au mieux, à suggérer un défaut.

## 1. Décisions qui en sortent

- **Runtime par défaut : sherpa-onnx** (format ONNX, même famille que la
  vision D81 ; couvre Parakeet, Canary, Whisper, SenseVoice, Qwen3-ASR…,
  plus VAD et diarisation ; meilleur rapport vitesse/qualité en CPU).
  Tranche la question ouverte n°1 du PRD.
- **whisper.cpp reste une option de build** (feature `whisper` /
  `whisper-vulkan`) : c'est le **meilleur chemin GPU** mesuré ici
  (Vulkan, toute marque de GPU, sans CUDA) et le format GGUF.
- Les deux peuvent cohabiter dans un binaire **seulement si sherpa est
  lié en bibliothèque partagée** (§4, piège 1).

## 2. Banc

| | |
|---|---|
| Machine | portable, Intel Core Ultra 7 255H (16 threads), 62 Go RAM ; GPU NVIDIA RTX PRO 500 Blackwell Laptop 6 Go (pilote 595.99, CUDA 13.2) + iGPU Intel Arc |
| Threads | 6 (plafond `jobs = 6` du dépôt) |
| Jeu de référence | **FLEURS fr_fr test** (Google, CC BY 4.0), 100 phrases distinctes, 1 009 s de parole lue, transcription normalisée fournie |
| Radio | 10 min de France Inter (Icecast `franceinter-midfi`, 2026-10-09 vers 21 h), découpées en segments de 30 s comme en prod (D160) ; pas de référence, sert au RTF et à la lecture qualitative |
| WER | corpus, normalisation `pnex_asr::wer` (minuscules, apostrophes unifiées, ponctuation et tirets = séparateurs) ; les nombres écrits en chiffres vs lettres comptent comme erreurs |
| Noms propres | rappel des mots capitalisés hors début de phrase de la référence brute (111 occurrences) — proxy grossier mais stable |
| RTF | temps de calcul / durée audio ; « flux ≈ 1/RTF » = flux temps réel soutenables en série sur la machine |

## 3. Résultats

Flux temps réel soutenables (≈ 1/RTF) et qualité. Le chiffre « radio »
est le plus représentatif de la prod (segments de 30 s).

| Modèle | Runtime | Matériel | Flux (FLEURS) | Flux (radio) | WER FLEURS | Noms propres | Chargement |
|---|---|---|---|---|---|---|---|
| Parakeet TDT 0.6B v3 int8 | sherpa | CPU | 25 | **32** | 8,7 % | 74 % | 1,6–3,9 s |
| Parakeet TDT 0.6B v3 int8 | sherpa | CUDA | 24 | 32 | 8,2 % | 76 % | 2–2,4 s |
| Canary 180M flash int8 | sherpa | CPU | 19 | 18 | 9,6 % | 72 % | 0,8–1,2 s |
| Canary 180M flash int8 | sherpa | CUDA | 18 | 16 | 9,6 % | 72 % | 1–1,1 s |
| Whisper small int8 | sherpa | CPU | 4 | 4 | 16,0 % | 63 % | 1–1,7 s |
| Whisper small int8 | sherpa | CUDA | 10 | 12 | 15,6 % | 65 % | 1,6–6,8 s |
| Whisper turbo int8 | sherpa | CPU | 4 | 6 | 8,4 % | 78 % | 2–19 s |
| Whisper turbo int8 | sherpa | CUDA | 6 | 11 | 8,2 % | 78 % | 2,2–2,5 s |
| Whisper small (GGML) | whisper.cpp | CPU | 4 | 5 | 14,8 % | 67 % | 0,3–0,9 s |
| Whisper small (GGML) | whisper.cpp | Vulkan | 31 | **35** | 15,0 % | 67 % | 0,4–0,6 s |
| Whisper large-v3-turbo q5_0 | whisper.cpp | CPU | 1 | 2 | **7,1 %** | **82 %** | 0,9–1,4 s |
| Whisper large-v3-turbo q5_0 | whisper.cpp | Vulkan | 18 | **30** | 7,2 % | **82 %** | 0,4–0,8 s |

Lecture :

- **CPU** : Parakeet v3 domine (≈ 30 flux, WER 8,7 %). Canary 180M est
  l'option légère (≈ 18 flux, modèle de 150 Mo), utile sur petite
  machine. Whisper en CPU est 5 à 30 fois plus lent pour une qualité
  équivalente ou moindre.
- **GPU** : whisper.cpp Vulkan + large-v3-turbo q5_0 donne **la
  meilleure qualité** (WER 7,1 %, 82 % des noms propres) à ≈ 30 flux
  radio sur un GPU de portable de 6 Go. C'est le candidat pour le
  worker GPU (D166).
- **sherpa + CUDA n'accélère pas les modèles int8** (Parakeet, Canary :
  temps identiques au CPU) ; il accélère Whisper int8 d'un facteur 2 à 3
  seulement. Variantes fp16/fp32 non testées (§5).
- Cible du PRD (§10 : ≥ 15 flux temps réel sur un GPU grand public) :
  **atteinte** — 30 flux en Vulkan, 32 flux en CPU seul avec Parakeet.
- Qualitatif radio : Parakeet v3 détecte la langue seul ; il transcrit
  aussi les **paroles des chansons** (anglais compris). Un filtrage
  musique/parole (VAD ou classifieur audio) sera nécessaire pour ne pas
  compter les paroles comme du contenu éditorial (lot 2).

## 4. Pièges rencontrés

1. **sherpa-onnx statique + whisper.cpp dans le même binaire = crash au
   chargement** (`free(): invalid pointer`). Cause non investiguée
   (conflit entre les bibliothèques C++ statiques). **Solution vérifiée :
   sherpa en partagé** (feature `sherpa-shared` + `SHERPA_ONNX_LIB_DIR`)
   à côté de whisper.cpp statique : les deux tournent dans un même
   process.
2. **Le build de `sherpa-onnx-sys` télécharge ses binaires depuis les
   releases GitHub sans vérifier de checksum.** Pour la prod et la CI :
   archive épinglée et vérifiée, fournie par `SHERPA_ONNX_ARCHIVE_DIR`
   ou `SHERPA_ONNX_LIB_DIR` (chaîne d'approvisionnement, R20).
3. **whisper.cpp prend le premier device Vulkan**, souvent l'iGPU :
   `gpu_device` doit être réglable (`WhisperOptions::gpu_device`,
   `--gpu-device 1` ici). Sur l'iGPU Intel, large-v3-turbo tournait à
   RTF 0,47 au lieu de 0,056.
4. **Build Vulkan** : il faut `glslc` (shaderc) et les en-têtes Vulkan ;
   le loader système `libvulkan.so` suffit. **CUDA (sherpa)** : build
   GPU officiel `sherpa-onnx-v1.13.8-cuda-13.x-cudnn-9.x-onnxruntime1.28.2`
   + bibliothèques CUDA 13 (cudart, cublas, cufft, curand) et cuDNN 9 ;
   aucun toolkit CUDA n'est installé sur la machine, tout vient d'un
   environnement conda-forge local (§6).
5. **Les exports Whisper ONNX de sherpa ne donnent pas de timestamps
   par mot** (« decoder model does not have cross-attention outputs ») ;
   il faut les ré-exporter avec `export-onnx-with-attention.py`.
   Parakeet et whisper.cpp en donnent nativement (D165 exige les mots
   horodatés).
6. **Premier chargement de Whisper turbo en CPU (sherpa) : 19 s**
   (contre 2 s ensuite, cache disque froid) : le check à l'import (D167)
   doit mesurer `load_ms` à chaud et à froid, ou le signaler.
7. **CUDA n'aide pas l'int8** dans onnxruntime (§3) : ne pas conclure
   « GPU inutile » sans tester fp16.

## 5. Non mesuré (à faire si besoin, pas avant)

- Raspberry Pi 5 réel (aucun chiffre ARM ici ; l'émulation QEMU ne
  donne pas de RTF exploitable).
- Variantes fp16/fp32 de Parakeet et Whisper sous sherpa + CUDA.
- whisper.cpp CUDA (nécessite le toolkit CUDA au build).
- Parallélisme réel : N flux simultanés (ici les segments passent en
  série ; « flux ≈ 1/RTF » est une borne, la saturation mémoire GPU à
  6 Go reste à mesurer).
- Radio annotée à la main (WER sur parole spontanée, débats) : FLEURS
  est de la parole lue, plus facile que la radio.
- VAD Silero (téléchargée, non branchée) et diarisation.

## 6. Reproduire

Données et modèles (≈ 4,5 Go) dans `~/.cache/pnex-asr-test/` :

```sh
R=~/.cache/pnex-asr-test; mkdir -p $R/models $R/data/fleurs $R/data/radio
B=https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models
cd $R/models
for n in sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8 \
         sherpa-onnx-nemo-canary-180m-flash-en-es-de-fr-int8 \
         sherpa-onnx-whisper-small sherpa-onnx-whisper-turbo; do
  curl -fL $B/$n.tar.bz2 | tar xj
done
curl -fLO $B/silero_vad.onnx
for m in ggml-small.bin ggml-large-v3-turbo-q5_0.bin; do
  curl -fLO https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$m
done
cd $R/data/fleurs
curl -fLO https://huggingface.co/datasets/google/fleurs/resolve/main/data/fr_fr/test.tsv
curl -fL https://huggingface.co/datasets/google/fleurs/resolve/main/data/fr_fr/audio/test.tar.gz | tar xz
# radio : 10 min, 16 kHz mono
ffmpeg -i http://icecast.radiofrance.fr/franceinter-midfi.mp3 -t 600 -vn -ac 1 -ar 16000 \
  -c:a pcm_s16le $R/data/radio/franceinter-10min.wav
```

Environnement GPU sans toolkit système :

```sh
G=$R/gpu
conda create -y -p $G/env -c conda-forge "cuda-cudart>=13,<14" "libcublas>=13,<14" \
  libcufft libcurand "cudnn>=9" shaderc vulkan-headers
curl -fL https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-cuda-13.x-cudnn-9.x-onnxruntime1.28.2-linux-x64-gpu.tar.bz2 | tar xj -C $G
```

Builds (toujours via le garde, un target par combinaison) :

```sh
# CPU, chaque runtime seul
CARGO_TARGET_DIR=target/sherpa-only  scripts/guarded.sh cargo build --release -p pnex-asr --features sherpa  --example asr_bench
CARGO_TARGET_DIR=target/whisper-only scripts/guarded.sh cargo build --release -p pnex-asr --features whisper --example asr_bench
# GPU : sherpa CUDA (partagé) + whisper.cpp Vulkan, dans le même binaire
L=$(ls -d $G/sherpa-onnx-*/lib)
PATH=$G/env/bin:$PATH SHERPA_ONNX_LIB_DIR=$L CARGO_TARGET_DIR=target/both \
  scripts/guarded.sh cargo build --release -p pnex-asr --features sherpa-shared,whisper-vulkan --example asr_bench
```

Lancer (une commande par modèle et par jeu, en série pour ne pas
fausser le RTF) :

```sh
M=$R/models
LD_LIBRARY_PATH=$L:$G/env/lib target/both/release/examples/asr_bench \
  --fleurs $R/data/fleurs --limit 100 --threads 6 --out $R/out/fleurs \
  --provider cuda --gpu --gpu-device 1 \
  --model sherpa:parakeet:$M/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8 \
  --model whisper:$M/ggml-large-v3-turbo-q5_0.bin
# radio : --wav $R/data/radio/franceinter-10min.wav --chunk 30 au lieu de --fleurs
```

La sortie est une ligne de tableau Markdown par modèle ; `--out` écrit
les hypothèses (`id`, référence, transcription) pour relecture.
