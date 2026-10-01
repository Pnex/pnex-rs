# Architecture — découpage des crates et features

> Phase 1. Décisions structurelles du workspace, à respecter dans toutes les
> phases suivantes.

## Le workspace

| Crate | Rôle | Cibles |
|---|---|---|
| `pnex-core` | DTO/constantes partagés backend ↔ frontend. **Aucune dépendance native** (pas de tokio, std::net, std::fs…). Serde uniquement. | natif **et** wasm32 |
| `pnex-backend` | API Loco/Axum, WS ingestion, worker, serving statique du front. Binaire `pnex-server`. | natif |
| `pnex-frontend` | Web UI Dioxus **CSR pur**. Appelle l'API Loco via HTTP (reqwest) et WS. | wasm32 (web) · aarch64 (android, debug) |
| `pnex-firmware-builder` | Lib d'orchestration des builds firmware (Phase 6). | natif |

## Le piège « fullstack Dioxus » — et pourquoi on ne l'utilise pas

Dioxus propose un mode `fullstack`/`ssr` avec **server functions** (`#[server]`)
et un serving Axum intégré au runtime Dioxus. Notre backend est **Loco**, qui
est aussi Axum. Activer les deux :

- **collision de servings** : deux stacks Axum/tower concurrentes pour une
  seule application — incompatibles à composer proprement ;
- **hydration SSR/CSR** : le rendu serveur exige que le binaire serveur et le
  wasm partagent exactement le même rendu — contrainte forte, fragile, et
  inutile pour un tableau de bord IoT derrière auth ;
- verrouillage sur le runtime Dioxus pour la partie serveur, alors que la
  valeur de Loco (ORM, migrations, workers, config) est côté serveur.

**Décision** : `pnex-frontend` est CSR web pur. Les features Dioxus
`server`/`fullstack` ne sont **jamais** activées. La communication avec le
backend est du HTTP/WS explicite, comme le ferait un client externe — même
contrat que les devices et les tests de parité.

## Cible Android — build validé, runtime à compléter

Directive (2026-08-15) : une app desktop/mobile PNEX est **prévue**. Sur ces
cibles le front n'est PAS servi par le backend (pas de same-origin) : l'URL
du serveur auto-hébergé est renseignée par l'utilisateur, « façon Bitwarden ».
L'architecture est déjà en place :

- `api/config.rs` — seam unique de résolution de la base URL : web = URLs
  relatives (same-origin, ou `PNEX_API_BASE_URL` à la compilation pour le
  dev hot) ; natif = env d'exécution puis préférence stockée
  (`pnex.api_base`) ;
- `pages/server_url.rs` — écran « URL du serveur » (actif natif uniquement :
  routé quand `api_base()` est vide, saisi + persistée via `storage::local()`,
  puis probe `GET {base}/` avant d'ouvrir le shell) ; sur la page de login,
  une section dépliable « Serveur auto-hébergé » (`SelfHostedSection`, façon
  Bitwarden — demande du 2026-09-09) permet de changer d'URL après coup :
  préremplie, probe + persistance au clic, sans quitter l'écran de connexion ;
- `storage.rs` — natif : store persistant dosé par un fichier JSON du
  dossier privé de l'app (`pnex-storage.json` — Android : files dir résolu
  par JNI brut, cf. `imp::app_files_dir`) : URL saisie, tokens et org
  survivent au kill ; le store session (verifier PKCE) reste volatil ;
- le login PKCE est réorientable (webview dédiée / schéma custom — à trancher
  en phase desktop/mobile).

### Chaîne de build validated (2026-09-08)

`task build:frontend:android` produit un APK debug arm64 (jniLibs arm64-v8a
seule — dx **accumule** les jniLibs des builds précédents dans le projet
gradle généré, la tâche nettoie donc le répertoire android avant chaque
build) : `cargo check` aarch64-linux-android vert du premier coup, assets
bundlés (tailwind, flasher, tron-gerbe, logos). Env SDK/NDK/JDK résolues
par les vars Taskfile (`ANDROID_HOME`, `NDK_HOME`, `JAVA_HOME`), surchargées
par l'env shell. dx build web n'est pas touché : `build:frontend` (wasm)
reste la référence pour le serving Loco.

### Runtime validé sur émulateur (2026-09-08)

APK debug installé et lancé sur l'émulateur `Medium_Phone_API_36.1` :
boot natif sans crash (le point `thread_local`/`!Send` ne mord pas au
lancement), écran `ServerUrl` rendu, probe `GET {base}/` contre le backend
(`10.0.2.2:5150` depuis l'émulateur = hôte) réussie, routeur → page de
login. Relance après kill → écran `ServerUrl` à nouveau (storage mémoire
d'alors — la persistance fichier du 2026-09-09 supprime ce comportement :
URL et session survivent désormais au kill).

### Login natif — navigateur système + pont backend (2026-09-08)

Implémenté (`auth/native.rs` + contrôleur `oauth2.rs` côté backend) :

- **Pourquoi pas de webview embarquée** : dioxus-desktop ouvre toute
  navigation http(s) dans le navigateur système (`webbrowser::open` codé en
  dur dans son wrapper de navigation, sans interrupteur) — la webview qui
  hébergerait les pages Rauthy est impossible sans fork.
- **Flow** : l'app génère le PKCE + `state` (le challenge), ouvre le
  navigateur système sur `{base}/api/v1/oauth2/sso` (proxy → Rauthy) avec
  `redirect_uri = {base}/api/v1/oauth2/native` ; Rauthy y rapatrie
  `?code&state` (page « retournez dans l'app » servie par le backend, qui
  mémorise `state → code` 5 min) ; l'app — en arrière-plan — sonde
  `GET /native/{state}`, échange le code (verifier local) et ouvre la
  session : l'utilisateur retrouve l'app déjà connectée au retour.
- **Sécurité** : le verifier PKCE ne quitte jamais l'app ; le `state` est
  le secret de capacité du pont (le code n'est servi qu'à qui le connaît).
  Rauthy reste l'IdP central : MFA et futurs IdP brokers passent tels quels.
- **Whitelist Rauthy** : chaque URL de déploiement
  `{base}/api/v1/oauth2/native` doit figurer dans les `redirect_uris` du
  client `pnex` (bootstrap `deploy/rauthy/clients.json` pour la 1re init,
  API admin sinon).
- **Rate-limit Rauthy** : toutes les sources de login passent par le
  backend → Rauthy ne voit que l'IP passerelle Docker (172.18.0.1) ;
  des tentatives répétées bloquent cette IP unique, et une requête pendant
  la fenêtre semble la prolonger (22 → 27 min, constaté le 2026-09-08) —
  ne rien toucher jusqu'à l'expiration. Dev : `enable_ip_block = false`
  dans `deploy/rauthy/config.toml` (`[login_delay]`, actif au prochain
  restart du conteneur) ; le délai progressif par compte reste actif.
  Réactiver `enable_ip_block` pour tout déploiement réel.
- **Boucle de dev émulateur et device physique** : le backend (:5150,
  bindé 0.0.0.0) est la base URL de l'app (`http://10.0.2.2:5150` depuis
  l'AVD, `http://192.168.1.16:5150` depuis un device sur le LAN). Rauthy
  sert **http (8080, appels serveur-à-serveur du backend) + https
  auto-signé (8443, navigateurs)** : l'issuer publié est
  `https://192.168.1.16:8443/auth/v1/` — un device physique a besoin d'un
  **contexte sécurisé** pour la page login Rauthy (`crypto.subtle`),
  impossible en http sur IP LAN. Le navigateur du device traverse
  l'avertissement de certificat une fois ; le backend appelle token/JWKS
  en HTTP local (`base_url`), l'issuer navigateur est `issuer_url`
  (settings.rauthy) — séparation nécessaire au certificat auto-signé. Si
  l'IP DHCP change : PUB_URL (compose) + issuer_url (development.yaml) +
  redirect_uri du client pnex (API admin).

### Validation E2E Android (2026-09-08) — login PKCE natif complet

Chaîne **validée de bout en bout sur l'émulateur** (API 36) : app →
`Sign in` → navigateur système (Chrome) → login Rauthy → pont backend
(« Connexion effectuée ») → polling de l'app → échange PKCE → session
ouverte → Dashboard avec données live. Deux validations successives.

Pièges corrigés en cours de route (tous documentés là où ils se corrigent) :

- **Crashs SIGABRT Android** (ndk-context double-init + `webbrowser::open`
  via macros jni) → crates **`ndk-context` et `webbrowser` vendus et
  patchés** ([patch.crates-io]) — cf. `vendor/patches/README.md`.
- **Cookies `__Host-`+Secure refusés par Chrome Android sur
  `http://localhost`** (adb reverse) → session d'authorize perdue →
  « Invalid credentials » en boucle : `COOKIE_MODE=danger-insecure` dans
  le compose (dev uniquement).
- **Clés de chiffrement Rauthy** : en DEV_MODE, la migration dev insère un
  JWKS chiffré avec la clé dev hardcodée upstream — le registre cryptr doit
  la contenir (`[encryption]` dans `deploy/rauthy/config.toml`), sinon
  l'émission de tokens échoue en `EncKey ID does not exist`.
- **Freezer Android** : l'app ET Chrome gelés en arrière-plan (cached app
  freezer, API 36) — le polling ne tourne que l'app au premier plan, et le
  code du pont expire côté Rauthy (~2 min) : revenir dans l'app rapidement
  après la page « Connexion effectuée ». Le test E2E automatisable devra
  en tenir compte.
- **« Exception » fantôme post-`startActivity` (API 36)** : `ExceptionCheck`
  répond vrai après l'ouverture du navigateur alors que `ExceptionOccurred`
  ne rend AUCUN throwable (état impossible selon la spec JNI, cause non
  élucidée) — le navigateur s'ouvre normalement. Le patch webbrowser
  inspecte l'état avant de trancher : seul un throwable réel
  `ActivityNotFoundException` (aucun navigateur) est une erreur ; le
  fantôme est logué `pnex-webbrowser:` dans logcat et classé non fatal
  (plus de toast trompeur). Cf. `vendor/patches/README.md`.
- **`adb shell am start` sur une app déjà lancée** peut recréer l'Activity
  dans le même process — avant le patch ndk-context, cela tuait l'app
  (assert `previous.is_none()`). `force-stop` avant `am start` dans les
  scripts.

Reste à faire pour une cible Android utilisable (ordre conseillé) :

1. **Flash** — Web Serial absent d'Android (Chrome comme WebView) : masquer
   le bouton « Flasher » (stubs `flash.rs` déjà en place).
2. **Branding APK** — identifier `com.example.PnexFrontend` placeholder ; à
   fixer via `[bundle]` Dioxus.toml avant toute diffusion.

### Persistance du storage natif (2026-09-09)

Le store persistant natif (`storage::local()`) est dosé par un fichier JSON
(`pnex-storage.json`) dans le dossier privé de l'app — Android : files dir
résolu par JNI brut sur `ndk_context` (même école que le patch webbrowser
vendu, sans panic ; échec → dégradé mémoire seule). Le trait `KeyValueStorage`
et tous les consommateurs inchangés : l'URL serveur saisie (ServerUrl), les
tokens (session Rauthy) et l'org courante survivent au kill de l'app ; le
verifier PKCE reste volatil (store session). Au boot, un refresh token
encore valide → `get_user_info` (401 éventuel → refresh single-flight) →
session rétablie sans ressaisie : plus de re-login à chaque ouverture.
Écritures atomiques (tmp + rename), fichier corrompu → démarrage à vide.

⚠️ Point connu pour cette phase : le client HTTP vit en `thread_local`
car les futurs reqwest sont `!Send` en wasm ; si le runtime mobile spawn
sur plusieurs threads et exige des futurs `Send`, il faudra rendre la couche
`Send` côté natif (cfg) ou changer d'organisation. L'activation des autres
cibles (`dioxus/desktop`, iOS) restera une **décision explicite de phase**
(le principe « pas de glissement silencieux » reste).

## Features du frontend

```toml
[features]
default = ["web"]
web = ["dioxus/web"]
router = ["dioxus/router"]
```

- `web` uniquement. Pas de feature `ssr`/`server` — le piège documenté ci-dessus.
- desktop/mobile : cf. section précédente — phase explicite ultérieure.

## Styling et i18n (Phase 3)

- **Tailwind CSS v4** : source `crates/pnex-frontend/style/tailwind.css`
  (`@import "tailwindcss" source(none)` + `@source "../src"`), générée vers
  `assets/tailwind.css` (gitignoré) par `task css:build` — **avant tout build
  dx** (toujours passer par la Taskfile). CI : step npm (`npm ci` +
  `npm run css:build`) dans le job `dx build`.
- **i18n Fluent** (`dioxus-i18n`) : locales `locales/{fr-FR,en-US}.ftl`
  embarquées, zéro libellé en dur, parité des clés testée
  (`i18n::tests::parite_cles_fr_en`).

## Serving du front

- `dx build --platform web` sort dans `target/dx/pnex-frontend/<mode>/web/public` ;
- le Taskfile copie ce dossier vers `crates/pnex-frontend/dist` (gitignoré) ;
- Loco sert `crates/pnex-frontend/dist` via le middleware `static` avec fallback
  `index.html` (SPA) — cf. `crates/pnex-backend/config/*.yaml`.

En boucle de dev front pure : `task dev:hot` (dx serve :5151 + `task css:watch`,
hot-reload). Le backend tourne à part (`task dev:backend` :5150) ; le CORS dev
verrouillé sur l'origine :5151 est dans `development.yaml` **uniquement**.

## Contrat d'API, versions et découverte LAN (2026-09-09)

Auto-hébergement LAN (Raspberry Pi + app Android sur le même réseau) :
l'app doit pouvoir **trouver** le serveur, afficher les **versions** des deux
côtés, et **refuser proprement** un désaccord de contrat au lieu de produire
des erreurs API opaques.

- **`crates/pnex-api-contract`** — source de vérité unique app ↔ serveur
  (serde seul, wasm-safe) : `CONTRACT` (bump = rupture d'API),
  `SERVICE = "pnex-server"`, `META_VERSION_PATH`, DTO `ServerInfo`,
  `compatible(a, b)` (égalité stricte). Backend ET front en dépendent.
  ⚠️ Les `sources` timestamp des tâches Taskfile `build:frontend`,
  `build:frontend:android` et `dev:frontend` incluent cette crate — y
  toucher reconstruit bien le front (sinon dist périmée après un bump).
- **`GET /api/v1/meta/version`** (`controllers/meta.rs`) — public, sans
  auth (école `health.rs`, préfixe `/api/v1/meta` obligatoire), sans CORS
  (front web same-origin, APK en sockets natifs hors WebView). Renvoie
  `ServerInfo { service, version, contract }` ; la version composée vient
  de la fn libre `app_version()` de `app.rs` (le hook `Hooks::app_version`
  délègue).
- **Porte de compatibilité** (front) — `state/compat.rs` (`GATE`) +
  `pages/gate.rs` : au boot et à chaque changement de serveur, l'app
  interroge le endpoint meta. Contrat différent → écran bloquant (versions
  et contrats des deux côtés) ; injoignable ou non-PNEX (404 d'un backend
  trop ancien compris) → écran bloquant + réessai + « Changer de serveur »
  (natif uniquement). `main.rs` branche `SERVER_READY` → `GATE` → routeur.
- **Scan LAN** (natif uniquement) — `pages/lan_scan.rs`, section embarquée
  dans `ServerUrl` et `SelfHostedSection` : sonde `http://{ip}:5150` sur un
  /24 (254 sondes, 48 en parallèle, timeout 1,2 s), ne retient que ce qui
  s'identifie `pnex-server`. Plage : dérivée de l'URL serveur courante si
  IP privée, sinon JNI `java.net.NetworkInterface` (aucune permission
  supplémentaire, miroir de `storage::imp`), sinon `192.168.1.` éditable.
  La connexion à un résultat passe par `apply_server` (`server_url.rs`) —
  chemin unique « probe + persistance + porte + `SERVER_READY` » partagé
  par les trois points d'entrée. Le scan n'existe pas en web : la page est
  servie par le serveur lui-même (same-origin) — trouver le serveur n'y a
  pas de sens, et la policy interdit le CORS hors dev.
- **Versions visibles** — `version.rs` (`CARGO_PKG_VERSION` + `BUILD_SHA`,
  même pattern que le backend) : pied de page de login (`login-app-version`)
  et carte « À propos » du profil (app + serveur, contrats).

## Règles de garde (à ne pas briser)

1. `pnex-core` (et désormais `pnex-api-contract`) doit toujours compiler sur
   les **deux** cibles — `task check` le vérifie, la CI l'impose.
2. Le backend ne dépend jamais du frontend ; le frontend dépend des crates
   partagées wasm-safe uniquement (`pnex-core`, `pnex-api-contract` — jamais
   du backend).
3. Aucune feature Dioxus serveur ne peut apparaître dans `pnex-frontend`.
