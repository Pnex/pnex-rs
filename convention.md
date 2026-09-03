# Conventions du projet pnex-rust

> Référence unique pour ne pas se perdre. Toute nouvelle décision de nommage
> ou de structure se consigne ici (et dans PROGRESS.md pour l'historique).

## Positionnement

- **La version Rust est la version officielle.** Django était un POC multi-années,
  jamais passé en prod → **pas de parité cosmétique** avec Django (slashs terminaux,
  formes d'URL héritées, etc.). L'API Rust fait référence.
- Ce qui DOIT être conservé de Django : les **contrats fonctionnels** capturés en
  Phase 0 (`docs/contracts/`) — payloads, sémantique, comportements.

## Noms

| Élément | Convention | Exemple |
|---|---|---|
| Service (nom affiché, health, logs) | `pnex-server` | `pnex_core::SERVICE_NAME` |
| Binaire backend | `pnex-server` | `cargo run --bin pnex-server` |
| Crates | préfixe `pnex-` | `pnex-core`, `pnex-backend`, `pnex-frontend`, `pnex-firmware-builder` |
| Répertoires de crates | **identiques aux noms de packages** | `crates/pnex-core/`, `crates/pnex-backend/`… |
| App Loco (`app_name`) | `pnex-server` | `crates/pnex-backend/src/app.rs` |

⚠️ Le nom `og-device-hub` (héritage Django) est **mort** — ne jamais le réintroduire.

## Chemins

- Front buildé par dx → `target/dx/pnex-frontend/<mode>/web/public`, copié par le
  Taskfile vers `crates/pnex-frontend/dist/` (gitignoré) — c'est ce dossier que
  Loco sert (middleware static + fallback SPA).
- Configs Loco : `crates/pnex-backend/config/{development,test,production}.yaml`.
- Chemins relatifs inter-crates : toujours via le nom complet
  (ex. `../pnex-frontend/dist`), jamais `../frontend`.

## Git & CI

- Une branche par phase : `phase-<N>-<slug>` (ex. `phase-1-squelette`).
- **Jamais de commit rouge** : `task check` (natif + wasm32), `task test` et
  `task lint` (clippy `-D warnings`) doivent passer avant tout commit.
- Push au fil de l'eau vers `origin` (github.com:Pnex/pnex-rs.git, opensource).
- Messages de commit en français, impératif, avec contexte de phase.

## Doc & langue

- Docs, commentaires et PROGRESS.md en **français**.
- `docs/architecture/features.md` : règles de features Dioxus (CSR web pur,
  jamais `fullstack`/`ssr`/`server`).
- `pnex-core` doit compiler sur natif **et** wasm32 — zéro dépendance native.

## Fluides : rien en base, tout au service externe

- **Pas de catalogue de fluides en DB** (directive) : les propriétés de
  fluides passent par le service FastAPI externe (CoolProp aujourd'hui,
  RefProp demain), qui est la **source de vérité**. En cas d'erreur, le
  message du service est **renvoyé tel quel au client** (pas de traduction).
- La base ne garde que les **mélanges custom** créés par les orgs
  (`fluid_mixtures`, `org_id` NOT NULL, composition JSONB structurée).
- Les tables Django `fluid_catalogs`/`fluid_property_groups` (miroir statique
  de FluidsList / config app) sont supprimées — le rendu vers la syntaxe du
  service fluids et les groupes de propriétés sont du code Rust, pas de la
  donnée. Garde-fou : `schema_invariants.rs` interdit leur retour.
- Le fluide d'une formule est nommé **dans l'expression même**
  (ex. `PropsSI('H','T',t,'P',p,'Water')`) — résolu à runtime par le service.

## Migrations (loco-rs `create_table` DSL)

- Toujours déclarer `("id", ColType::PkAuto)` en tête des colonnes — la DSL
  n'ajoute **pas** le PK automatiquement.
- Référence nullable = suffixe `?` sur le **nom de la table référencée**
  (1er élément du tuple) : `&[("organizations?", "org_id")]` → colonne
  nullable + `ON DELETE SET NULL`. Référence obligatoire (sans `?`) →
  `CASCADE`. **Ne pas redéclarer la colonne FK dans `cols`** : la déclaration
  écraserait la colonne générée (type correct) mais perdrait la FK.
- Le nom de table référencée passe par `normalize_table` (pluriel cruet) :
  passer `"user"` ou `"users"` est équivalent ; la colonne dérivée est
  `user_id`.
- Index uniques composites/partiels : SQL brut `execute_unprepared`, noms
  **sous-tirets** (`uniq_…`, `idx_…`) — les tirets ne sont valides que dans
  les identifiants quotés par sea-query (contraintes FK générées).
- Invariants structurels garantis par `migration/tests/schema_invariants.rs`
  (nullabilité org_id, actions ON DELETE, absence des tables de copie) —
  à étendre à chaque invariant structurant.

## Auth (Phase 3, IdP Rauthy depuis D19)

- **Validation JWT locale par JWKS** (pas d'introspection) : RS256 uniquement,
  `iss` et `aud` vérifiés explicitement (durcissements vs Django POC),
  audience acceptée = `{client_id, "account"}`. Rafraîchissement des JWKS
  quand un `kid` inconnu apparaît (rotation de clés). **Issuer Rauthy =
  `{base}/auth/v1/` avec slash final** — la validation `jsonwebtoken` est un
  match exact, l'omettre casse toute validation. La JWKS Rauthy mélange RSA
  et OKP/EdDSA : le parser ignore les entrées sans `n`/`e`.
- **Refus par défaut** : un endpoint qui prend l'extracteur `AuthUser` répond
  401 sans token valide — pas de permission AllowAny implicite.
- **JIT provisioning** (`auth/provisioning.rs`) : première requête authentifiée
  crée en une transaction `users` + `user_profiles` + org personnelle
  (owner, tier Free). Resynchronise email/nom si changés côté IdP. `sub`
  Rauthy = 24 caractères (pas un UUID) → colonne `users.idp_sub` (varchar) ;
  re-liaison par email si le `sub` change (migration d'IdP). Access tokens
  Rauthy lean (pas de `preferred_username`) → `display_name` retombe sur
  l'email.
- **Scoping org** : l'extracteur `OrgContext` (`X-Org-Id` + membership vérifié)
  est le point d'ancrage du multi-tenant — les contrôleurs ne filtrent jamais
  « à la main » par user.
- **Rôles API en minuscules** (`owner`, `admin`, `viewer`) en entrée comme en
  sortie — les enums SeaORM générés sérialisent en Capitalized, on mappe via
  `controllers::orgs::role_str`/`RoleParam` (ne pas éditer `_entities/`).
- **Tests sans IdP** : `tests/common/` fournit un mock JWKS (axum, port
  aléatoire, route `/auth/v1/oidc/certs`) + une clé RSA de test
  (`tests/fixtures/jwks_test_key.pem`, sans valeur). `RAUTHY_URL` pointé
  dessus avant le boot. Base de test :
  `TEST_DATABASE_URL`, vidée entre tests par le hook `truncate`
  (`dangerously_truncate` dans config/test.yaml).

## Front (Phase 3 — port de l'UI `pnex-ui`)

- **i18n obligatoire, zéro libellé en dur** : tout texte visible passe par
  `t!("clé")` (Fluent via `dioxus-i18n`, wrapper `src/i18n.rs`). Locales
  `locales/{fr-FR,en-US}.ftl` embarquées, fallback en-US, **parité des clés
  obligatoire** entre locales. Résolution : localStorage (`pnex.locale`) >
  `profile.language` (après login) > `navigator.language` > en-US.
- **Serveur** : le web est **same-origin** (front servi par Loco, URLs
  relatives, jamais de sélection de serveur). La config d'URL serveur
  « façon Bitwarden » n'existe que pour les futures cibles
  desktop/ios/android : seam unique `api/config.rs` + clé `pnex.api_base` +
  écran `pages/server_url.rs` (non routé). Le dev hot (`task dev:hot`, dx
  :5151) utilise `PNEX_API_BASE_URL` à la compilation + CORS dev verrouillé
  dans development.yaml (jamais en prod).
- **Tailwind v4** : source `style/tailwind.css` (`@source "../src"`), généré
  vers `assets/tailwind.css` (gitignoré) par `task css:build` **avant tout
  build dx** (toujours passer par la Taskfile). La macro `asset!()` exige le
  fichier **à la compilation** : `task check`/`test`/`lint` dépendent de
  `task css:ensure` (stub vide si le CSS réel est absent — jamais l'inverse),
  et la CI génère le même stub dans les jobs check/test (le job dx build
  produit le CSS réel via npm). Le scanner lit du texte brut :
  les classes conditionnelles sont des **littéraux complets** (`match` qui
  retourne deux chaînes entières), jamais `format!("bg-{c}-100")`.
- **Client HTTP** (`api/client.rs`) : reqwest en `thread_local` (futurs
  `!Send` en wasm — la cible desktop devra les rendre `Send`). Bearer +
  `X-Org-Id` (lu depuis le signal `ORG`, jamais sur `/oauth2/*`), refresh 401
  single-flight + retry unique, échec → expiration de session + toast.
  **Messages d'erreur relayés tels quels** au client (detail > message >
  bloc error/description), pas de traduction.
- **Toasts** : `ToastMessage::Text` (message serveur relayé tel quel) vs
  `ToastMessage::Key` (clé i18n traduite à l'affichage) — jamais de texte
  UI en dur stocké dans un toast.
- **État global** (`state/`) : `SESSION`, `ORG`, `TOASTS` en `GlobalSignal`.
  Mutation depuis une fn : méthode intrinsèque `with_mut(&self)` (les setters
  du trait Writable exigent `&mut`, impossible sur un static). L'org courante
  est persistée (`pnex.org`) via `state::org::set` — seule voie d'écriture.
- **Routing** : routes **statiques** uniquement ; le détail d'org est porté
  par le signal `ORG` (les props de route ne redémarrent pas `use_resource`).
  Pas de route `/login` : le shell rend `Login` à la place de l'Outlet
  (parité `AuthWrapper` React). `WebHistory` fourni par défaut par dioxus-web.
- **Login** : PKCE redirect uniquement (verifier/challenge S256, verifier en
  sessionStorage, consommé au callback). Register/reset = `action=`
  (pages UI Rauthy : `/auth/v1/users/register`, `/auth/v1/account` — le
  changement de mot de passe vit dans l'IdP). Logout = end-session Rauthy
  `/auth/v1/oidc/logout` avec `id_token_hint`. Password grant (dev/tests) :
  Rauthy exige l'**email** comme username ; ses refresh tokens portent
  `nbf = exp_AT − 60` → refresh immédiat rejeté par design (l'UI ne
  rafraîchit que sur 401).



- URLs **sans slash terminal** (convention Rust/Axum, assumée).
- Endpoints préfixés par domaine (`/health/*`, puis `/api/...`, `/ws/...`).
- Ne jamais inventer un endpoint ou un champ : en cas d'ambiguïté sur un contrat
  Django, vérifier `docs/contracts/` ou demander.
