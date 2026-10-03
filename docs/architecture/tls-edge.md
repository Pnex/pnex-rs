# TLS edge — TLS partout, y compris sur Raspberry Pi (D70)

> Décision utilisateur 2026-09-27. Un reverse proxy **nginx** termine
> TOUT le TLS (navigateurs, APK, devices) sur **une seule origine**
> `https://<domaine>`. Certificats automatiques : CA locale (mode `local`)
> ou Let's Encrypt (mode `cloud`).

## Pourquoi nginx (et pas Traefik / Caddy / un proxy Rust)

Contrainte dure : l'**ESP8266** (BearSSL) ne dispose que d'un tampon de
réception de 2 Ko (`setBufferSizes(2048, 512)` dans `pnex_tls.cpp`). Il
faut que le serveur envoie des records TLS ≤ 2 Ko, ce qu'obtient
l'extension *max_fragment_length* (MFLN, RFC 6066).

Mesures du 2026-09-27 (client `openssl s_client -maxfraglen 512`,
téléchargement de 600 Ko type OTA) :

| Serveur | MFLN acquitté | Plus gros record | ESP8266 |
|---|---|---|---|
| nginx (OpenSSL) | oui | ≤ 512 négocié | OK |
| HAProxy (OpenSSL) | oui | ≤ 512 négocié | OK |
| Traefik v3.6 (Go) | non | 16 344 o | KO |
| Caddy (Go, même pile crypto/tls) | non | 16 Ko attendus | KO |
| rustls brut | non | 8 216 o | KO |
| rustls `max_fragment_size = 1024` | non (inutile) | 1 043 o | OK |

La voie « Traefik + passthrough SNI vers un listener rustls plafonné »
fonctionnait mais a été jugée trop complexe. nginx : une brique, tout
passe au vert, dépendance OpenSSL confinée à l'image.

## Topologie

```
                   :443 nginx (edge-nginx)
navigateur / APK ──►  /auth/v1/*  ─► rauthy:8080  (HTTP, PROXY_MODE, DEV_MODE=false)
device (wss)     ──►  /ws/*       ─► pnex-server :5150 (upgrade websocket, timeout 1 h)
                      /*          ─► pnex-server :5150
                   :80  ─► 301 https (sauf /.well-known/acme-challenge/)
```

- **Une origine** : plus de CORS, plus de port 8443, plus de
  `COOKIE_MODE=danger-insecure`, issuer `https://<domaine>/auth/v1/`.
- **Nom stable, jamais de `/etc/hosts`** : défaut = `<hostname>.<domaine
  de recherche>` quand le DNS de la box le connaît (Livebox : `.home`,
  enregistré au bail DHCP — DNS classique, suit les changements d'IP,
  marche dans tous les navigateurs et sur les téléphones), sinon repli
  mDNS `<hostname>.local`. Constat 2026-09-27 : `.local` **non résolu**
  par Chromium/Brave sur un poste où avahi et systemd-resolved se
  disputent mDNS (`ERR_NAME_NOT_RESOLVED`), et DoH des navigateurs l'ignore
  → `.local` n'est qu'un repli. Sur un Pi : nommer la machine `pnex`
  donne `https://pnex.home` (ou `pnex.local`). Le nom stable rend
  `rauthy:lan` (URIs par IP) inutile en mode edge.
- **Nom collant** : une fois choisi, `PNEX_DOMAIN` est relu depuis
  `edge.env` à chaque `edge:up` (auto-détection au tout premier seulement).
  Constat 2026-09-27 : la Freebox a « oublié » `shan-hapster.home` (entrée
  DNS liée au bail DHCP) → une relance a basculé sur `.local`, issuer et
  client OIDC compris. Le DNS de la box n'est **pas** une base fiable pour
  les devices.
- **Origine par IP** (`PNEX_DOMAIN=192.168.1.185`, adoptée 2026-09-27 après
  la 2e disparition du nom `.home`) : TLS + OIDC OK (cert SAN IP, CA
  importée) ; WebAuthn interdit un `rp_id` IP → `apply-edge.sh` pose
  `PNEX_RP_ID=localhost` (passkeys indisponibles, mot de passe OK). `.local`
  inutilisable ici : avahi publie aussi les bridges docker (résolu en
  `172.19.0.1`).
- **IP LAN dans le certificat** (mode local) : chaque IPv4 globale de la
  machine est ajoutée en `IP:` (navigateurs) **et** `DNS:` (mbedTLS 2.x de
  l'ESP32 ne compare que les dNSName). Les devices s'enregistrent par IP
  nue (ce que renvoie le scan LAN) → aucune dépendance DNS ; réservation
  DHCP recommandée pour la machine edge.
- Alias `<hostname>.local` (mode local, s'il diffère du nom canonique) :
  ajouté aux SAN de la feuille et **redirigé 301** par nginx vers
  `https://<domaine canonique>` — une seule origine pour le client OIDC et
  les cookies.
- Rauthy : port HTTP publié en **loopback seulement** (appels
  serveur-à-serveur du backend) ; `TRUSTED_PROXIES` = bridge docker.
  `DEV_MODE=false` obligatoire : en dev mode Rauthy exige un port dans
  `PUB_URL` et l'issuer deviendrait `https://<domaine>:443/auth/v1/`.
- TLS 1.2 conservé (BearSSL n'a pas TLS 1.3). Suite négociée côté
  ESP8266 simulé : `ECDHE-ECDSA-AES256-GCM-SHA384`, MFLN acquitté.

## Certificats

**Mode `local`** (`edge-pki`, one-shot alpine+openssl, à chaque `edge:up`) :

- CA racine ECDSA P-256, 10 ans, générée **une seule fois**
  (`deploy/edge/pki-data/ca/`, gitignoré). Jamais régénérée
  automatiquement : les firmwares l'épinglent.
- Feuille P-256, 397 jours (plafond Apple), SAN = domaine + localhost +
  127.0.0.1 + `PNEX_EXTRA_SANS`. Ré-émise si absente, < 30 jours, SAN
  changés ou CA différente. nginx recharge à chaud (`40-cert-watch.sh`,
  md5 toutes les 30 s).
- `fullchain.pem` = feuille seule (la racine est l'ancre de confiance).
- Import clients : `task edge:trust` (Linux : store système + NSS
  Chrome/Firefox) ; autres clients : `deploy/edge/pki-data/ca.pem`.

**Mode `cloud`** (`--profile cloud`, `ACME_EMAIL` requis) : certbot
HTTP-01 webroot, renouvellement toutes les 12 h, copie vers
`pki-data/current/` → rechargement nginx. `edge-pki` ne pose qu'un
placeholder auto-signé pour le premier démarrage. Module ACME natif de
nginx écarté : absent de l'image officielle et sans DNS-01.
**Non validé en réel** (pas de domaine public au moment de la décision).

## Exploitation

| Task | Effet |
|---|---|
| `task edge:up` | génère `deploy/edge/edge.env`, lance pki + nginx (+ certbot), recrée Rauthy derrière le proxy, étend le client OIDC `pnex` en live, vérifie la discovery |
| `task dev:backend` | lit `edge.env` (dotenv, fichier absent ignoré) → `RAUTHY_ISSUER_URL=https://<domaine>` |
| `task edge:trust` | importe la racine sur la machine locale (sudo) |
| `task app:up` | si `edge.env` existe (edge actif), passe **toujours** par `apply-edge.sh` : un `compose up` nu recréerait Rauthy/pnex-server sans les surcharges TLS (constat 2026-09-27 : « Sign in » → `http://rauthy:8080`) |
| `task edge:down` | retire nginx/pki/certbot, supprime `edge.env`, Rauthy revient en dev (8080/8443) |

Variables : `PNEX_DOMAIN`, `PNEX_EDGE_MODE` (`local`|`cloud`),
`PNEX_EDGE_BACKEND` (`host`|`container`, D71),
`PNEX_EXTRA_SANS`, `ACME_EMAIL`, `ACME_STAGING`,
`PNEX_BACKEND_UPSTREAM` (calculé : `pnex-server:5150` ou IP de la
passerelle compose + `:5150`).

Backend : le `redirect_uri` de repli du pont OAuth respecte
`X-Forwarded-Proto` (sinon `http://` derrière le proxy → refus Rauthy).

## Limitation de débit et IP client (2026-10-01)

Le backend limite le débit des routes non authentifiées / sensibles
(`services/rate_limit.rs`, couche axum posée dans `after_routes`) :
`/api/v1/oauth2/*` (60/min, sondage du pont natif 240/min),
`/api/v1/agent/enroll` (10/min, code historique `agent-enroll-rate-limited`),
`/ws/device`, `/ws/sensor/ingest`, `/ws/camera*` (300/min),
`/api/v1/public/tours/*` (600/min). Fenêtre fixe, compteurs **dans Valkey**
(script Lua `INCR`+`PEXPIRE`, partagés entre pods) ; sans Valkey ou en cas
de panne, compteurs locaux par pod. Refus = **429** `rate-limited` +
`Retry-After` (secondes).

IP client : l'adresse **socket** du pair, sauf si ce pair est un proxy de
confiance — alors le saut le plus à droite non fiable de
`X-Forwarded-For` (repli `X-Real-IP`). nginx ajoute `$remote_addr` en fin
de `X-Forwarded-For` : un en-tête forgé par le client reste à gauche et
est ignoré. IPv6 regroupé par /64.

| Réglage | Défaut | Effet |
|---|---|---|
| `PNEX_TRUSTED_PROXIES` / `settings.rate_limit.trusted_proxies` | `127.0.0.0/8, ::1, 10/8, 172.16/12, 192.168/16, fc00::/7` | CIDR des proxies dont on lit les en-têtes ; `none` = n'en croire aucun |
| `PNEX_RATE_LIMIT=off` / `settings.rate_limit.enabled: false` | activé | coupe la couche |

Le défaut « réseaux privés » couvre nginx en compose, l'ingress k8s et le
réseau pod. Un backend exposé **directement** sur un LAN sans proxy doit
poser `PNEX_TRUSTED_PROXIES=none` (sinon un hôte du LAN peut forger
`X-Forwarded-For` pour contourner sa limite).

## Validé (2026-09-27, mode local)

- Discovery : issuer `https://<domaine>/auth/v1/` ; token password grant
  `iss` identique, accepté par le backend.
- Login PKCE navigateur headless complet (landing → Rauthy 2 étapes →
  callback → tokens), `isSecureContext = true`, zéro exception de cert
  hors SPKI de test.
- Upgrade websocket `/ws/device` : 101 à travers nginx.
- Handshake type ESP8266 (TLS 1.2, MFLN 512, vérif CA + hostname) : OK.

## Apps natives (APK) — confiance TOFU (2026-09-28)

- **Scan LAN** : chaque hôte du /24 est sondé en parallèle sur
  `https://{ip}` (edge, préféré) et `http://{ip}:5150` (repli) ; un hit
  http masqué si le même hôte répond en https. La sonde https ne vérifie
  pas le certificat (lecture de la carte d'identité publique seulement,
  jamais de token).
- **Connexion** (`apply_server`) : sonde stricte (webpki + CA épinglée).
  Échec en https → téléchargement de `/api/v1/meta/ca` sans vérification,
  contrôle que cette CA **valide bien** le certificat servi, puis dialogue
  « Faire confiance ? » avec l'empreinte SHA-256 (à comparer avec Profil →
  À propos sur le web). Accepté → CA stockée (`pnex.server_ca`), tous les
  clients reqwest la font confiance, connexion rejouée.
- **Login** : la page Rauthy s'ouvre dans le navigateur système — il faut
  y installer aussi la CA (bouton « Télécharger le certificat » du
  dialogue, puis Paramètres → Sécurité → Installer un certificat → CA),
  sinon avertissement de sécurité à accepter.
- CA régénérée côté serveur → la sonde stricte échoue → nouveau dialogue
  avec la nouvelle empreinte (jamais de bascule silencieuse).

## Reste à faire (suite D70)

1. **Firmware** : ~~case ws/wss~~ **fait (2026-09-27)** — case supprimée
   (wizard, modal rebuild, page Référentiels), `ws_ssl` forcé à `true` côté
   serveur (écriture des hôtes + lancement des builds), scan LAN → IP nue
   (le device passe par l'edge sur 443). **`PNEX_CA_CERT` injectée
   automatiquement (2026-09-27)** : pki-init écrit `device-ca.pem` (CA
   locale ; ISRG Root X1 téléchargée en cloud), le worker la lit via
   `PNEX_CA_CERT_FILE` (edge.env en mode hôte, montage `/pki` du
   `pnex-builder`). Historiquement obligatoire sur ESP32 (ArduinoWebsockets
   ne passait jamais le client en insecure) ; depuis le client maison
   `pnex_ws` (2026-10-03), sans CA = `setInsecure` sur les deux cœurs, et la
   CA est aussi appliquée sur la WS ESP8266 (avant : insecure forcé). Sur
   ESP8266, BearSSL vérifie les dates : `pnex_tls` lance SNTP et passe
   `setX509Time` (heure NTP, sinon date de build du firmware). Pas de NTP requis sur
   ESP32 (mbedTLS sans `HAVE_TIME_DATE` : dates non vérifiées).
   Bascule ws → wss par OTA : le téléchargement suit l'ancien transport
   (http :5150, toujours publié) — un device dont le firmware wss n'a pas
   la CA reste hors ligne → re-flash USB. Reste : E2E réel ESP8266 (heap
   BearSSL ; WS en insecure, CA seulement pour l'OTA).
2. **UI** : ~~page de téléchargement de la racine CA + QR code~~ **fait
   (2026-09-28)** — endpoint public `GET /api/v1/meta/ca`
   (`META_CA_PATH`, `.crt` en `application/x-x509-ca-cert`, 404 sans edge
   ou en mode cloud) ; carte « Certificat racine du serveur » dans Profil
   → À propos : empreinte SHA-256, lien de téléchargement, QR code (scan
   depuis le téléphone). `pnex-server` conteneurisé : `PNEX_CA_CERT_FILE`
   + `PNEX_EDGE_MODE` + montage `/pki` (compose.app.yaml).
3. **APK** : ~~`network_security_config`~~ **fait (2026-09-28)** —
   `patch-android-manifest.py` ajoute un `<base-config>` system + user au
   fichier généré par dx (WebView / navigateur). Le client HTTP Rust
   (rustls + webpki-roots) n'utilise pas ce fichier : il **épingle la CA
   de l'edge** (TOFU) — voir « Apps natives » ci-dessous.
4. **Image backend** (Dockerfile) pour un compose de déploiement complet
   Pi/cloud — aujourd'hui le backend tourne sur l'hôte.
5. Durcissement éventuel : `nameConstraints` sur la CA locale (limite
   l'impact d'une fuite de clé) — écarté en v1, support mbedTLS/BearSSL
   à vérifier.
