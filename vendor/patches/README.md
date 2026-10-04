# Patches locaux (mécanisme `[patch.crates-io]`)

Deux crates vendus et patchés, câblés dans le `Cargo.toml` racine :

```toml
[patch.crates-io]
ndk-context = { path = "vendor/patches/ndk-context" }
webbrowser  = { path = "vendor/patches/webbrowser" }
```

## Pourquoi

Le 2026-09-08, la validation Android du frontend Dioxus (`pnex-frontend`,
feature `mobile`) a buté sur une série de crashs SIGABRT (« PnexFrontend
keeps stopping ») à l'ouverture du navigateur système par le login SSO.
Tombstones analysés via `adb logcat -b crash` :

1. **`ndk-context::initialize_android_context` — `assert!(previous.is_none())`**
   (`Java_dev_dioxus_main_WryActivity_create`) : un second `onCreate` dans le
   même process (relance explicite de l'app — ex. `adb shell am start` sur
   une app déjà lancée —, deep-link, réglage « ne pas garder les activités »)
   ré-initialise le contexte → assert → abort. Le glue tao
   (`ndk_glue.rs`) appelle `initialize_android_context` à chaque `create`
   et n'efface le contexte qu'au `destroy`.

2. **`jni` 0.22.4 — `Expected an exception after ExceptionCheck`**
   (env.rs:706, via `webbrowser::open`) : dioxus-desktop appelle
   `webbrowser::open` **en dur** pour toute navigation http(s) de sa webview
   (`webview.rs:386`) et pour tout IPC `<a>` externe (`app.rs:279`), et
   l'implémentation Android du crate webbrowser passe par les macros
   haut-niveau du crate `jni`, dont le chemin d'exception asserte
   (`exception_occurred().expect(...)`) et abort l'app dès qu'une exception
   Java traverse la séquence (une panic Rust ne peut pas retraverser une
   frontière JNI → `panic_cannot_unwind` → SIGABRT).

## Quoi

- **`ndk-context/`** (0.1.1) : `initialize_android_context` écrase
  silencieusement (plus d'assert), `release_android_context` idempotent, et
  ajout de `try_android_context() -> Option<AndroidContext>` (variante
  non-paniquante).
- **`webbrowser/`** (1.2.4) : implémentation Android réécrite en JNI brut
  (`jni::sys`, table de fonctions, appels non-variadiques `*A`, frame local
  `Push/PopLocalFrame`) : `ExceptionCheck`/`Clear` systématiques, tout en
  `Result` — plus aucun panic possible, y compris en l'absence de navigateur
  (`ActivityNotFoundException` → `Err` propre). `src/android.rs` seul
  modifié, sections `[[test]]`/`[dev-dependencies]` retirées du manifest
  (tests amont non exécutables depuis l'emplacement vendu ; ils tiraient
  actix/tokio dans `cargo test --workspace`).

## Effet de bord assumé

`webbrowser::open` reste le point de passage unique de l'ouverture de
navigateur : dioxus-desktop (webview + IPC `<a>`) et `pnex-frontend`
(`auth/native.rs`, login SSO) l'appellent tous les deux — la version vendue
rend les trois chemins sûrs sans fourker dioxus-desktop ni tao.

### Exception post-`startActivity` (API 36) — classée, pas masquée

Sur l'émulateur API 36, `startActivity` est régulièrement suivi d'un
`ExceptionCheck` vrai alors que le navigateur s'ouvre normalement (constaté
à chaque login SSO). Cas piégeux mesuré le 2026-09-08 au soir :
`ExceptionOccurred` ne rend alors **AUCUN throwable** — un état fantôme
impossible selon la spec JNI (artefact ART, cause exacte non élucidée). Le
patch inspecte donc l'état réel avant de trancher :

- throwable `android.content.ActivityNotFoundException` → `Err` (aucun
  navigateur : vraie erreur, toast dans l'app) ;
- throwable réel mais classe illisible → `Err` prudent ;
- **fantôme** (check vrai sans throwable) ou throwable d'une autre classe →
  exception nettoyée, `Ok(())` + trace `pnex-webbrowser: …` dans logcat
  (non fatal, pas de toast).

## Mise à jour

Si bump de dioxus/wry/tao : re-vérifier que les crashs sont corrigés amont
et supprimer ces patches. Les deux crates sont minuscules ; le diff est
commenté `PATCH PNeX (date)`.

## wry 0.53.5 (2026-09-09)

**Symptôme** : SIGABRT au retour de l'appareil photo Android (intent caméra =
app en arrière-plan → pipe IPC du webview suspendu ; à la reprise, une
requête de protocole custom timeout) — `rx.recv_timeout(MAIN_PIPE_TIMEOUT)
.unwrap()` panique (`panic_cannot_unwind` dans `handleRequest`, école
[wry #1551](https://github.com/tauri-apps/wry/issues/1551)).

**Fix** (portage sur 0.53.5 du commit amont a535fd95/#1699) : timeout ×3 +
`.map(Some).inspect_err(trace).ok().flatten()` — une réponse vide au lieu
d'un abort. Fichier : `wry/src/android/mod.rs` (chemin du protocole custom).

Câblage : `wry = { path = "vendor/patches/wry" }` dans `[patch.crates-io]`.
Le fix est intégré en amont ≥ 0.54 — à retirer quand dioxus monte wry.

Second patch (2026-10-04) : feature `pnex-automation` (désactivée par
défaut) dans `wry/src/webkitgtk/web_context.rs`. Le contexte WebKitGTK
n'autorise une session WebDriver (WebKitWebDriver, suite e2e Linux) que si
la feature est compilée — uniquement via la feature `e2e` de pnex-frontend
(`task build:frontend:linux:e2e`) — **et** que `PNEX_E2E_AUTOMATION` est posé
au lancement. Aucun build distribué ne l'active (revue sécurité D130). Le
retrait du fix #1551 ne doit pas emporter ce patch : le garder tant que la
suite `e2e/tests-linux` existe.

## coolprop/ (mécanisme différent : patch C++ au build)

`coolprop/exception-guards.patch` ne fait **pas** partie du mécanisme
`[patch.crates-io]` (ce n'est pas un crate Rust). C'est un patch aux sources C++ du
submodule `vendor/CoolProp` (voir `vendor/README.md` § CoolProp), appliqué par le
`build.rs` de `crates/pnex-coolprop-sys` au moment du build — jamais commité dans le
submodule.
