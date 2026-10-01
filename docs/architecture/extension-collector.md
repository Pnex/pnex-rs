# Extension navigateur « type device » — spécification (C1b)

> Statut : **spécification** (amendement edge-model 2026-09-15 : pas de
> domaine « collecteurs » dédié ; la collecte web à deux types — C1a = nœud
> flow `http_fetch`, livré ; C1b = cette extension). **Zéro code tant que la
> spec n'est pas validée** ; l'implémentation est le jalon C3 de la roadmap
> (`edge-model.md` §11).

## 1. Objet et périmètre

L'extension Chrome/Firefox collecte le web **JS-lourd** (SPA, dashboards
tiers, pages animées) inaccessible au nœud `http_fetch` (C1a couvre le cas
« page statique / API JSON »). La spec couvre : identité, auth, transport,
format des règles, exécution in-browser, mapping vers les séries O2, cycle
de vie MV3, sécurité, consentement.


**Principe cardinal** (amendement §8) : l'extension réutilise l'identité et
le pattern **devices** — pas de domaine « collecteurs » (aucune table
source/binding), pas de JWT user dans le plan de données (edge-model §8.2
décision 7 : « jamais le JWT Rauthy user en collecteur » ; l'app mobile est
l'exception OIDC sanctionnée, pas l'extension).

## 2. Rappel doctrine (edge-model §8)

| | Firmware | App mobile | Extension | Agent |
|---|---|---|---|---|
| Contexte | MCU non supervisé | poche de l'user | navigateur de l'user | machine de l'user |
| Confiance | totale au serveur | user via OIDC | user-délégué | user via token |
| Présence | connexion WS permanente | à l'usage | éphémère (navigateur ouvert) | daemon + file disque |
| Survit coupure réseau | profils autonomes : oui | n/a | non (léger) | oui |

Doctrine MV3 déjà consignée : `chrome.alarms` >= 30 s + jitter,
config-as-data sans eval, **cast != apply** avec consentement par tâche,
extension user-déléguée tab-open v1, sélecteur CSS = pin.


## 3. Deux plans : gestion vs données

L'extension vit sur **deux plans séparés** :

- **Plan gestion** (rare, authentifié user) : login OIDC PKCE,
  choix/création du device, écriture des règles. API org classique
  (Bearer JWT + `X-Org-Id`).
- **Plan données** (fréquent, silencieux) : sync des règles (desired-state)
  + ingestion des valeurs. Identité **device** (token + clé de chiffrement),
  jamais le JWT user.

La « petite page de login » = la page d'options de l'extension : elle sert
le plan gestion. Une fois le device créé et le token stocké, la page n'est
plus nécessaire tant que le token vit.

## 4. Identité et auth

**Création** : `POST /api/v1/devices` (JWT + `X-Org-Id`, `can_write`),
payload `CreateDevice { device_id, predefined_device_name, metadata }` ;
le **201** renvoie le DTO complet incluant
`device_token { token, encryption_key, is_active, created }`.
`device_id` suggéré : `ext_<handle>-<slug>` (chaîne libre, l'admission
Tier 2 est permissive). Révocation : `is_active=false` ou
`DELETE /devices/{id}` (purge token) — école existante.

**Stockage local** (décision 8) : `chrome.storage.session` — mémoire du
navigateur ouvert, jamais localStorage ; re-pairing accepté v1 après
redémarrage du navigateur.


## 5. Transport (décision 1)

**WS `/ws/device` canal unique** : le canal existe, chiffré ChaCha20
(framing `base64(nonce 12 o || keystream)`, pas d'AEAD — D8), et le
**re-cast desired-state à l'announce** (`ws_device.rs` : ProvisionAck →
re-push des `Subscribe` persistés → re-cast `ControlConfig`) rend la sync
des règles gratuite côté serveur. Zéro changement serveur pour le
transport.

REST fallback : **différé** — aucun endpoint REST de télémétrie n'existe ;
en créer un = un 2e chemin d'ingestion à sécuriser. Si le service worker
MV3 s'avère trop hostile au pattern connect-on-wake (§9), un
`POST /devices/{id}/ingest` (token device, école `internal_notify`)
restera l'option de repli documentée.

**Crypto client** (décision 2) : **WASM de `pnex-core`** — le crate est
déjà compilable `wasm32` (proto + framing ChaCha20 partagés, golden
vectors communs avec le firmware). Implémentation pure-JS en secours
documentée (libs matures), au prix d'une 2e implémentation du framing.


## 6. Règles : écriture, stockage, sync

### 6.1 Où vivent les règles (décision 3)

Nouvelle entité org **`extension_rules`** : `{id, org_id,
device_registry_id, match (origin/pattern d'URL), selector (CSS),
attr? (text|attr|html), interval_ms (>= 30 000 + jitter), enabled}`.
CRUD REST `/api/v1/extension-rules` (`can_write`). Source de vérité côté
serveur ; l'extension n'exécute que ce que le serveur lui pousse
(desired-state — l'inverse serait un chemin de contrôle user non consenti).

Rejets argumentés :
- `regulator_configs` : projection de **flow déployé** (slot unique
  (device, flow, node), GPIO-centrique) — ne peut pas porter des règles
  génériques.
- `POST /devices/{id}/commands` : GPIO-centrique + 409 si device hors
  ligne — inadapté.

### 6.2 Format de sync (décisions 4-5-6)

Nouveau variant **`ServerMsg::RulesConfig { cmd_id, rules: Vec<RuleSpec> }`**
— additif, zéro impact firmware : le serveur n'envoie `RulesConfig` qu'aux
devices extension (l'ESP ne le recevra jamais ; à vérifier à
l'implémentation que le désérialiseur du firmware tolère un variant
inconnu — non bloquant, question §12).


`RuleSpec` (école ControlSpec, struct plate) :

```
RuleSpec { rule_id, origin, selector, attr?, interval_ms, enabled }
```

- `rule_id` : identifiant stable de la règle (clé de série O2, §7).
- `origin` : pattern d'URL (ex. `https://exemple.tld/*`) — l'exécution ne
  tourne que sur une page dont l'URL matche.
- `selector` : sélecteur CSS = **pin** (D48).
- `attr` : `text` (défaut) | `attr:<nom>` | `html` — ce qu'on lit.
- `interval_ms` : >= 30 000 (contrainte MV3 chrome.alarms) ; le jitter est
  ajouté côté extension.
- `enabled` : la règle désactivée n'est plus castée.

Ack d'apply = `DeviceMsg::Ack { cmd_id, ok, err }` (RPC existant).

## 7. Exécution in-browser et mapping séries O2

Sélecteur CSS = pin ; **device = le device extension** (device_id
`ext_…`) ; clé de série = `rule_id` sanitizé (école `device_payload_key`,
casse conservée). L'extension envoie des `StateReport` standard :

```
StateReport { gpio: rule_id, value: "valeur lue", ... }
```

— l'ingestion serveur existante fait le reste (série O2, diag). Une valeur
non numérique est signalée, pas droppée silencieusement (écart assumé avec
l'ingestion firmware : le DOM est textuel, le champ `attr` décide).


## 8. Consentement : cast != apply (décision 10)

Recevoir des règles n'applique rien. L'**apply** est par tâche, consenti et
journalisé :

- déclencheur `tab-open` : l'user ouvre l'onglet → la règle s'applique sur
  cette page (user-délégué, v1) ;
- déclencheur `interval` : `chrome.alarms` >= 30 s + jitter — l'extension
  ouvre/attache un onglet seulement si `host_permissions` couvre l'origin
  **consentie à l'installation de la règle** (prompt par origin, jamais
  `<all_urls>` en v1) ;
- chaque apply est journalisé localement (onglet, règle, ts) et rapporté
  via l'ingestion diag.

## 9. Cycle de vie MV3 : connect-on-wake (décision 9)

Le service worker MV3 est éphémère — pas de WS persistant v1. Pattern :

```
wake (alarm >= 30 s + jitter, ou tab-open, ou on-navigate)
  → connect WS /ws/device?token=…&device_id=…
  → Announce { chip: "extension", board: <chrome|firefox>, fw: version, caps: ["extension"] }
  → ProvisionAck (admission Tier 2 permissive)
  → drain : Subscribe persistés + RulesConfig (re-cast desired-state)
  → Ack cmd_id
  → exécution des règles dues (alarmes dues, onglet ouvert)
  → StateReport (valeurs lues)
  → disconnect
```

Compatible serveur **sans changement** : c'est exactement la séquence
announce → re-cast que `ws_device.rs` exécute à chaque reconnexion d'un
device (un reflash/reconnect retrouve ses configs sans EEPROM).


## 10. Sécurité

- Plan données : device token **jamais** le JWT user ; révocable à tout
  moment (`is_active`, DELETE), revalidé en session par le serveur.
- Secrets en `chrome.storage.session` (jamais localStorage).
- MV3 : pas de code distant (CSP), config-as-data (les règles sont des
  données sérialisées, jamais du JS évalué).
- `host_permissions` par origin consentie (pas de wildcard v1).
- Anti-injection : le contenu lu vient du DOM (texte/attribut), jamais
  interprété ; `html` ne fait que capturer, pas exécuter.
- Journal d'apply consultable dans la page d'options.

## 11. Non-goals v1

- Pas de REST ingest fallback (décision 1 : différé).
- Pas de `chrome.storage.local` pour les secrets.
- Pas de wildcard `<all_urls>`.
- Pas de provider de scraping (c'est C1a, côté serveur).
- Pas de WS persistant v1 (service worker éphémère).
- Firefox : WebExtensions compatibles visées, mais le développement et les
  tests v1 ciblent Chrome MV3 ; Firefox suit en C3.

## 12. Questions ouvertes (non bloquantes pour la spec)

- Tolérance du firmware à un variant `ServerMsg` inconnu (théoriquement
  sans objet : le serveur n'envoie `RulesConfig` qu'aux devices extension).
- Format exact du `StateReport` pour les valeurs textuelles (le contrat
  actuel porte des valeurs numériques floatées au bord — l'extension voudra
  peut-être porter du texte brut ; décision à prendre à l'implémentation
  C3, école `ts_source` : soit champ `raw` additif, soit droppé).
- UX de pairing (QR code comme l'app mobile ? saisie manuelle de l'URL +
  sélection du device ?).

## 13. Plan de tests et acceptation

- Vectors proto réutilisés (`pnex-core` golden vectors) pour le framing
  WASM — même référence que le firmware.
- Device « ext » de staging : séquence announce → drain → report rejouée
  par un harnais node (mêmes assertions que `tests/ws_device.rs`).
- E2E UI : écrire une règle dans PNeX → la voir castée à l'announce →
  valeur lue visible en Visualisation (`device_id=ext_…`).

## 14. Récapitulatif des décisions

| # | Décision | Choix |
|---|---|---|
| 1 | Transport | WS `/ws/device` unique ; REST fallback différé |
| 2 | Crypto client | WASM `pnex-core` (framing partagé, golden vectors) |
| 3 | Stockage des règles | Nouvelle entité org `extension_rules` + CRUD `can_write` |
| 4 | Format de sync | Nouveau variant `ServerMsg::RulesConfig` (additif) |
| 5 | `RuleSpec` | `{rule_id, origin, selector, attr?, interval_ms, enabled}` |
| 6 | Ack d'apply | `DeviceMsg::Ack { cmd_id }` existant |
| 7 | Login | OIDC PKCE (plan gestion) / device token (plan données) |
| 8 | Persistance locale | `chrome.storage.session` |
| 9 | Cycle de vie | Connect-on-wake (pas de WS persistant v1) |
| 10 | Consentement | Cast != apply par tâche, host_permissions par origin |
