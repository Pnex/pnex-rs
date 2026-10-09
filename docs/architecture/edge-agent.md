# Edge agent (`pnex-agent`) — C2 livré (D95–D99)

> Statut : **livré** (branche `feat/edge-agent`, 2026-09-30). Jalon C2 de
> `edge-model.md` §8.3 / §11. Exemples d'ingestion : `examples/edge-agent/`.

## 1. Rôle

Un binaire Rust installé sur une machine (PC, serveur, Raspberry Pi, Windows)
qui fait office de **serveur d'ingestion local** :

```
scripts locaux ──HTTP 127.0.0.1:7070──▶ API axum ─(mpsc borné)─▶ writer (group-commit redb, seq)
                                                                     │
                                          file disque redb ◀─────────┘
                                                │ lecture ordonnée
                              uplink ── Batch{epoch, points[seq]} ─wss + ChaCha20─▶ /ws/device
                                     ◀── BatchAck{up_to_seq} ── (purge de la file)
                                     ◀── AgentConfig{max_batch, max_keys}
```

- **API locale sans mot de passe** : `POST /v1/points` (point, tableau,
  objet plat, lignes `key=value`), `POST /v1/points/{key}`, `GET /v1/status`,
  `GET /v1/metrics`, `GET /healthz`. `202` = point **écrit sur disque (fsync)**.
- **Buffering automatique** : lien lent, saturé ou coupé → la file disque
  grossit, l'uplink rattrape (fenêtre de 4 lots en vol, lots ≤ 500 points /
  64 KiB, backoff exponentiel + jitter 1 s → 60 s).
- **Parallélisme** : runtime tokio multi-thread ; les requêtes HTTP
  concurrentes sont fusionnées par un thread writer unique (une transaction
  redb pour N requêtes) ; l'uplink lit en MVCC en parallèle des écritures ;
  `503 + Retry-After` quand le disque ne suit pas (backpressure).
- Crate : `crates/pnex-edge-agent` (lib + bin `pnex-agent`).

## 2. Décisions

### D95 — Un agent est un device

`predefined_device = edge_agent`, `device_type = agent`, même registre, même
token + clé partagée du lien Noise (D156, identique aux devices ; D8 retiré le 2026-10-08), même tunnel
`/ws/device`, même anti-clone 4003, même revalidation 4005, même liveness.
L'`Announce` porte `chip = "agent"` : **pas d'admission de pins, pas de
manifeste**, l'enregistrement (token valide) est l'admission. Gardes
serveur : build firmware, OTA et commandes de pins → `400
agent-unsupported-action`.

### D96 — Ingestion libre, Valkey toujours, O2 en opt-in par clé

- **Aucun catalogue** : n'importe quelle clé, n'importe quelle valeur JSON,
  unité optionnelle par point. Clé normalisée (`normalize_measurement_name`,
  D16), **découverte** à la première vue (`agent_keys` + miroir
  `discovered_measurements` pour les pickers existants). Seul garde-fou : un
  quota de clés distinctes (`max_unique_measurements`, 1000 par défaut,
  éditable dans l'UI) contre l'explosion de cardinalité.
- **Toute valeur va dans le cache live Valkey** (flows, mémoire, dashboards
  live). L'**historique OpenObserve** est activé par clé (`record_o2`,
  décoché par défaut) ou par point (`"record": true`).
- Mécanique : `TelemetryPoint.record` ; `ValkeyTapSink` écrit toujours, le
  sink O2 ignore `record = false`. Nombres/booléens → série métrique
  (`source_type = edge_agent`) ; texte/objets → clé Valkey sœur
  `pnex:lastj:v1:…` et, si enregistré, stream de logs O2 `ev_agent_events`.
- TTL Valkey d'une clé non enregistrée : 7 jours (pas de repli O2 ; le TTL
  reste un GC, la fraîcheur est décidée par `resolve`).

### D97 — Protocole `Batch` / `BatchAck`, horodatage d'origine, dédup

- `DeviceMsg::Batch{epoch, points[{seq, key, value, ts_ms, unit?, record?}]}`
  et `ServerMsg::BatchAck{epoch, up_to_seq}` / `ServerMsg::AgentConfig` —
  additifs, `Deserialize` manuel (piège `arbitrary_precision`).
- `epoch` = id aléatoire du fichier de file (persistant) ; `seq` strictement
  croissant, jamais réutilisé. **Dédup serveur** par high-water mark
  `(device, epoch)` dans Valkey (`pnex:agent:hwm:v1:…`, repli mémoire de
  session) → at-least-once + dédup = exactement-une-fois effectif (D46
  appliqué tel quel).
- `ts_source = device` : l'horodatage de capture est conservé (borné à
  +5 min dans le futur). Écriture Valkey **« set if newer »** (script Lua)
  : un point rejoué ne remplace jamais une valeur plus fraîche.
- Limite assumée : l'ack part après enqueue dans le sink (avant le flush O2).

### D98 — Enrôlement par code à usage unique + CA épinglée

- L'UI génère un code `XXXX-XXXX-XXXX` (alphabet sans ambiguïté, 60 bits,
  15 min, **seul son SHA-256 est stocké**, un code actif par agent).
- `POST /api/v1/agent/enroll` (public, rate-limité 10/min/adresse, compteur
  partagé entre pods via Valkey — cf. tls-edge.md) consomme
  le code de façon atomique et **fait tourner token + clé** : une
  réinstallation déconnecte l'ancienne machine (4005).
- Confiance TLS : `--ca-sha256` = empreinte de `/api/v1/meta/ca` affichée
  par l'UI ; l'agent télécharge la CA sans vérification, compare
  l'empreinte, puis n'accepte plus que cette CA (rustls, provider ring).
  Sans empreinte → racines web publiques (mode cloud).
- Fichiers : `config.toml`, `secrets.json` (0600 / ACL SYSTEM+Administrators),
  `ca.pem`, `queue.redb`, `logs/`.

### D99 — Distribution servie par l'instance, API locale en loopback

- Binaires **servis par pnex-server** (`/api/v1/agent/download/{target}` +
  `SHA256SUMS`, `PNEX_AGENT_DIST_DIR`) : Linux x86_64 / aarch64 / armv7 en
  **musl statique** (toute distro, Raspberry Pi), Windows x86_64 (mingw).
  Build : `task build:agent` (`deploy/agent/build-dist.sh`, zig épinglé) ;
  image : stage `agent-dist` du Dockerfile.
- Installation en une commande (Linux `curl … | sudo sh`, Windows PowerShell
  admin) : empreinte CA vérifiée **avant** tout téléchargement de script,
  binaire contrôlé contre `SHA256SUMS`, puis `pnex-agent install`
  (systemd système avec utilisateur dédié `pnex-agent`, `--user` sans root,
  ou service Windows LocalSystem).
- API locale : `127.0.0.1:7070` par défaut ; ouverture LAN = opt-in
  (`--listen 0.0.0.0:7070 --allow 192.168.1.0/24`), toujours sans mot de
  passe (la sécurité réseau vers PNeX reste le tunnel chiffré).

## 3. Limites et suites

- Windows : cross-compilé et vérifié à la compilation, **pas encore testé sur
  une vraie machine Windows** (service SCM, ACL `icacls`, one-liner PS 5.1/7).
- macOS : code prêt (pas de service launchd automatisé), non distribué (SDK).
- O2 rejette les points plus vieux que `ZO_INGEST_ALLOWED_UPTO` (défaut 5 h) :
  la compose de dev passe à 168 h ; **à reporter dans `pnex-deploy`**.
- Suites : pont WS pour devices LAN (§8.3 « futur »), commandes descendantes
  (`Command` D88), AEAD (au même moment que les devices).
