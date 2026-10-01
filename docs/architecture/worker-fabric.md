# PRD — Fabric de workers PNEX (control plane + workers, join-by-token)

**Statut :** Proposé — refonte de la stratégie worker (2026-09-25)
**Portée :** Infrastructure transverse (pré-requis de la feature Gaussian Splatting et de tout futur worker lourd)
**Backend :** Rust (Loco), cohérent avec l'archi PNEX existante
**Renvois :** `roadmap.md` (P1.6 / P2.7, axe C), `ml-vision.md` (pilier ML/Vision, jobs GPU), `media.md` (D21, splats), `firmware-build.md` (worker build firmware), `flow-engine.md` (SoT en base projetée au runtime)

---

## 1. Contexte & problème

PNEX exécute déjà du travail asynchrone via des **workers** : stitching 360 et build firmware. Aujourd'hui ces workers tournent **co-localisés sur le control plane** (y compris un Raspberry Pi) sans souci, parce qu'ils sont CPU-only.

La feature Gaussian Splatting (et plus largement le pilier AI/ML : SfM, vision, prédiction) introduit une contrainte nouvelle : **ces jobs exigent un GPU réel et ne peuvent pas tourner sur le control plane léger.** Le worker doit pouvoir vivre **ailleurs** que le control plane — sur le PC GPU d'à côté, une VM dédiée, un cluster on-prem, ou un service partagé.

Ajouter un « type gpu-worker » en parallèle des workers actuels créerait deux chemins de code et, à terme, la prolifération de microservices qu'on veut éviter. **La bonne réponse est de généraliser** : un runtime worker unique, où les *capabilities* (CPU/ARM, GPU Vulkan, VRAM…) différencient les rôles et pilotent le routage. Les workers 360/firmware deviennent le cas dégénéré (co-localisé, capability CPU) du même modèle ; le gpu-worker n'est qu'un profil de capabilities de plus.

Cette fabric doit être **scalable as-code (GitOps) et as-UI (Dioxus)**, traverser des topologies réseau variées (du tout-sur-une-machine à l'air-gap multi-nœuds), et rester simple à rejoindre — modèle mental cible : **les runners GitLab CI**.

## 2. Objectifs / non-objectifs

**Objectifs**
- Un seul runtime worker pour tous les types de job (light co-localisé → GPU distant).
- Routage des jobs par **capabilities** (le worker annonce, le job exige, le scheduler matche).
- Enregistrement d'un worker distant par **join-by-token**, façon runner GitLab CI.
- Traversée réseau propre selon la frontière de confiance (mesh privé vs API authentifiée).
- Pilotage **as-code** (Helm/ArgoCD) **et as-UI**, sur une source de vérité unique.
- Rétrocompatibilité : les workers 360/firmware existants entrent dans la fabric sans réécriture fonctionnelle.

**Non-objectifs**
- Devenir un fleet manager généraliste (provisioning de VM, orchestration multi-cloud). La fabric **enregistre et route** ; elle ne **provisionne** pas l'infra.
- Installer les drivers GPU de façon turnkey cross-OS (voir risques).
- Le worker Windows en priorité (Linux/K8s d'abord).
- Le billing/quota du service partagé (relève du PRD monétisation).

## 3. Principe directeur

Repris de la stratégie produit : **le choix de topologie doit venir d'une contrainte utilisateur, jamais d'une imposition.**

- La *localisation* d'un worker est une **config**, pas une variante de code.
- Un worker light et un worker GPU partagent le **même binaire** ; seules leurs capabilities diffèrent.
- La seule différence légitime entre « self-host » et « service partagé » : **qui opère et qui paie le GPU** — iso-features strict.

## 4. Modèle conceptuel (analogie runner GitLab CI)

| GitLab CI | PNEX Fabric | Notes |
|---|---|---|
| Runner | Worker | Même binaire PNEX, mode `worker` |
| Registration/auth token | Join token | Single-use, court-terme, scopé |
| Tags | **Capabilities** | Réutilise le modèle capabilities PNEX existant |
| Job tags → runner tags | Job requirements → worker capabilities | Le scheduler matche |
| Long polling (sortant) | Polling lent (sortant) | NAT-friendly, aucun port entrant |
| Executor (shell/docker/k8s) | Executor (natif/subprocess/conteneur) | COLMAP en subprocess, Brush en lib |
| Shared / group / specific | Scope tenant/projet | Pour le multi-tenant |

Un **worker = sac de capabilities** : `cpu`, `arch:arm64|amd64`, `gpu.vulkan`, `vram>=8g`, `has:colmap`, `has:ffmpeg`, `feature:splat|stitch360|firmware`. Un **job** déclare ses exigences ; le scheduler route vers n'importe quel worker qui les annonce. C'est le même modèle capabilities déjà retenu côté devices — un node compute est juste un autre porteur de capabilities.

## 5. Topologies bénies (archetypes)

Un petit jeu fixe de topologies « bénies », packagées en presets. Elles partagent le code ; elles ne le forkent pas.

| Archetype | Control | Worker(s) | Storage | Comm | Cible |
|---|---|---|---|---|---|
| **All-in-one** | local | in-process | FS local (pas de RustFS) | — | laptop, PC unique, petite install |
| **Split LAN** | Pi | PC GPU voisin | RustFS + PG sur le PC | mesh privé | prosumer / maker |
| **Control léger + worker distant** | Pi ou petite VM | GPU ailleurs | endpoint S3 + PG joignables via mesh | mesh privé | « boîte 24/7 + GPU à la demande » |
| **Cluster on-prem** | déploiement K8s | pool GPU (HPA) | S3/Ceph + PG existants | intra-cluster | gros industriel, air-gap |
| **Compute managé** | chez le client | worker = notre service | notre S3 | **API job authentifiée** | GPU-pauvre par abonnement |

Matrice **feature → pré-requis → archetype** (extrait) :

| Feature | GPU | S3 partagé | Connectivité | Archetypes possibles |
|---|---|---|---|---|
| Build firmware / stitch 360 | non | non (co-loc) | — | tous, y compris all-in-one Pi |
| Gaussian Splatting | **Vulkan** | **oui si split** | mesh ou API | tous sauf all-in-one Pi |

Règles de pré-requis (à tenir fermement) :
- **Plancher GPU = Vulkan**, pas CUDA/ROCm. Brush (wgpu) tourne sur NVIDIA / AMD sans ROCm / Intel Arc. CUDA n'accélère que COLMAP (fallback CPU) → nice-to-have, jamais un gate.
- **Split des plans ⇒ un endpoint S3**, pas RustFS-le-produit. RustFS = défaut recommandé ; l'industriel réutilise son S3/Ceph. Jamais une dépendance dure.
- **All-in-one ⇒ ni RustFS ni PG remote** : FS et DB locaux, un seul binaire.

## 6. Traversée réseau & frontières de confiance

**Le mode de comm suit la frontière de confiance** — c'est la règle unique qui organise tout le spectre :

- **Même propriétaire des deux côtés** (mon Pi + ma VM, mon cluster) → **mesh privé WireGuard**. Dedans, le worker atteint **PostgreSQL (queue Loco native) + RustFS en direct**, zéro exposition publique. On réutilise le worker Loco tel quel, aucun nouveau service. **Jamais** de Postgres exposé sur Internet.
- **Frontières différentes** (notre compute managé servant le control plane d'un client, ou l'inverse) → **API de job HTTP authentifiée**, sans DB partagée. C'est le seul cas où un contrat réseau explicite vaut le coût — et c'est ce qui sépare proprement « worker distant perso » de « service partagé ».

Dans tous les cas, **le worker poll le control plane** (sortant) — jamais l'inverse. Un worker derrière NAT n'a besoin que de connectivité sortante ; aucune découverte de worker à gérer côté control plane.

## 7. Enregistrement — join-by-token

Modèle runner GitLab CI, avec la sécurité d'un auth-token créé-puis-lié (pas un registration-token longue durée) :

1. Le control plane émet un **join token** : single-use, court-terme, scopé. Il embarque l'adresse du plane, l'endpoint + **creds S3 scopés**, la config WireGuard, et les capabilities visées.
2. Le worker s'installe, rejoint le mesh, **s'enregistre en annonçant ses capabilities**, puis démarre le polling.
3. Le control plane le voit apparaître dans la fleet (état, capabilities, heartbeat).

### 7.1 Générateur de script d'install
Le control plane génère un script d'install préconfiguré pour la cible : **Debian (systemd)**, **Windows (service)**, ou **manifest Kubernetes**. Le script pose l'app + la config, rejoint le mesh, s'enregistre et démarre.

Garde-fous :
- Le script **contient des secrets** (creds S3, token, clé WG) → single-use, court-terme, scopé, jamais de creds root longue durée. Traité comme un secret.
- **Driver GPU = la partie sale.** Le générateur pose fiablement app + config + join ; pour les drivers GPU il **vérifie et guide**, il ne force pas de module kernel. Pas de promesse turnkey driver.
- Le générateur **s'arrête à générer + join.** Il ne devient pas un fleet manager (pente vers les 50 microservices).

## 8. Dispatch & fiabilité

Les jobs durent des minutes à des dizaines de minutes → le **polling lent** est le bon choix, pas un compromis. Aucune brique push/broker nécessaire. On réutilise la **queue Postgres de Loco** (pattern worker firmware).

- **Claim atomique** : `SELECT … FOR UPDATE SKIP LOCKED` (ou `UPDATE … WHERE status='queued' … RETURNING`) — deux workers ne prennent jamais le même job. Fourni par Loco ; à ne pas rater si hand-roll un jour.
- **Détection de worker mort** (le seul point que le polling ne donne pas gratuitement, critique vu la durée des jobs) : **lease + heartbeat**. Le worker rafraîchit `heartbeat_at` pendant le job ; un **reaper** requeue les jobs dont le lease a expiré. **À valider explicitement dans Loco** — les libs de queue sous-traitent souvent mal le crash-recovery.
- **Progression ≠ queue** : la courbe SSIM/steps live passe par un **canal séparé** — le worker écrit ses métriques dans **OpenObserve**, l'UI lit là. « Dispatch lent » ne veut jamais dire « UI qui rame ».

## 9. Scalable as-code & as-ui

Source de vérité unique = **PostgreSQL** (même modèle que les flows : SoT en base, versionné, projeté au runtime). Les deux surfaces pilotent la même base :

- **As-code (GitOps)** : pools de workers, capabilities cibles et config déclarés en fichiers ; déploiement via **Helm/ArgoCD** (cohérent avec l'infra PNEX). Le déploiement crée les workers et les lie via token ; le HPA scale les pools GPU.
- **As-UI (Dioxus)** : voir la fleet (workers, capabilities, santé, jobs en cours), **générer un join token / un script d'install**, créer/éditer des pools, inspecter la queue, drainer/retirer un worker.

Les deux écrivent via l'API Loco → aucune dérive entre config déclarée et état réel.

## 10. Migration des workers existants

- **360 stitch** et **firmware build** deviennent des **profils de capabilities** (`feature:stitch360` / `feature:firmware`, `cpu`, `arch:arm64`) dans la fabric unifiée. Aucun changement fonctionnel.
- **All-in-one (Pi self-host)** = worker **in-process**, capabilities CPU — le cas dégénéré du même modèle. Rétrocompatible.
- **gpu-worker** = nouveau profil (`gpu.vulkan`, `vram>=…`, `has:colmap`) — aucun chemin de code parallèle.

## 11. Modèle de données (Postgres)

- **`worker`** : `id`, `tenant_id`, `name`, `status` (`online|draining|offline`), `capabilities` (JSONB), `last_heartbeat_at`, `registered_at`.
- **`worker_pool`** : `id`, `tenant_id`, `name`, `target_capabilities` (JSONB), `desired_size`, `managed_by` (`gitops|ui`).
- **`join_token`** : `id`, `tenant_id`, `scope` (JSONB : capabilities, pool, S3 scope, WG), `expires_at`, `used_at` (single-use).
- **`job`** (extension) : `required_capabilities` (JSONB), `claimed_by` (worker), `lease_expires_at`, `heartbeat_at`.

## 12. Phases

- **MVP** : runtime worker unifié + capability routing ; queue PG existante ; **un** archetype distant (control léger + worker GPU distant via mesh WG) ; join token + enregistrement par capabilities ; lease/heartbeat/reaper ; 360 & firmware repliés dans la fabric.
- **v2** : générateur de script d'install (Debian → K8s → Windows) ; UI de gestion de fleet ; pools GitOps + HPA.
- **v3** : frontière managée (API job authentifiée), scoping multi-tenant, archetype compute-managé (relie au PRD monétisation).

## 13. Risques & questions ouvertes

- **Install driver GPU cross-OS** : scope honnête (vérifier/guider, pas forcer).
- **Worker Windows** : nid à galères (COLMAP, Vulkan, service) → Linux/K8s d'abord, ou via Docker.
- **Secrets dans le join token / script** : rotation, single-use, révocation.
- **Tuning du lease** sur jobs longs : trop court = faux zombies requeue ; trop long = zombie réel qui traîne.
- **Ne pas glisser vers un fleet manager** : la fabric enregistre et route, elle ne provisionne pas.
- **Découverte/adressage du compute** : réduire « rien / LAN / on-prem / partagé » à **un seul champ de config + fallback gracieux** quand aucun compute n'est joignable (sinon 4 chemins de code).
