# PNeX end-to-end suite

Playwright tests that drive the real web UI against a running stack. They
complement `cargo test` (units, API, DB): here a scenario goes through the
browser exactly as a user would, and the API is only used to seed
preconditions, check what the UI persisted, and clean up.

```bash
task e2e:install                 # once: bun deps + Chromium for Playwright
task app:up                      # or: task dev — any reachable stack
task e2e                         # whole suite (en + fr projects)
task e2e:smoke                   # every route renders, both locales
task e2e -- --grep @flows        # one area (tags below)
task e2e -- --headed --workers=1 # watch it
cd e2e && bun run report         # HTML report of the last run (traces on failure)
```

## Target and session

| Variable | Default |
|---|---|
| `PNEX_E2E_BASE_URL` | `https://<PNEX_DOMAIN>` when `deploy/edge/edge.env` exists, else `http://localhost:5150` |
| `PNEX_E2E_ISSUER` | `<base>/auth/v1/` (`http://localhost:8080/auth/v1/` for the plain dev server) |
| `PNEX_E2E_EMAIL` / `PNEX_E2E_PASSWORD` | `admin@example.com` / `admin-pass` (Rauthy bootstrap user) |
| `PNEX_E2E_ORG` | `E2E` — organization the suite owns |
| `PNEX_E2E_FRESH_ORG=1` | delete and recreate that org before the run |
| `PNEX_E2E_KEEP=1` | skip the start-of-run sweep (keep what the last run left) |
| `PNEX_E2E_WORKERS` | `2` |
| `PNEX_E2E_DPR` | `2` (device scale factor; `1` = faster runs) |
| `PNEX_E2E_BASEMAP_URL` / `PNEX_E2E_BASEMAP_DARK_URL` | `https://map.alpine-box.com/style/light-en` / `…/dark-en` (public MapLibre styles used as test basemaps) |
| `PNEX_E2E_GEO_BASE` | unset — root of a live Photon + GraphHopper (`<root>/photon`, `<root>/graphhopper`); `@geo-live` tests are skipped without it. Holds a secret path segment: CI secret, never committed |

No test logs in through the Rauthy pages except `auth.spec.ts`: a password
grant gives the tokens, injected into `localStorage` before the app boots
(`pnex.access_token`, `pnex.refresh_token`, `pnex.org`, `pnex.locale`).

**The suite never touches the operator's organization.** It works in its own
org (found or created by name), and the global setup empties its functions,
flows and dashboards at the start of each run. Resources a test creates are
named with a unique prefix (`prefix` fixture) so parallel tests never collide.

## Writing a test

```ts
import { expect, test } from '../src/fixtures.ts';
import { FunctionsPage } from '../src/pages/functions.ts';

test('…', { tag: '@functions' }, async ({ app, api, prefix, capture }) => {
  const fns = new FunctionsPage(app);
  await fns.open();
  const editor = await fns.create(`${prefix} demo`, 'starlark', 'threshold');
  await capture('editor', { caption: 'Threshold template' });
  expect((await editor.testRun({ value: 25, threshold: 20 })).outputs.alarm).toBe('true');
});
```

Fixtures (`src/fixtures.ts`):

| Fixture | What |
|---|---|
| `app` | `AppShell` on an authenticated page: `goto`, `t(key)` / `tr(key)` (ftl lookups), toasts. Fails the test on any page error or wasm panic |
| `api` | REST client scoped to the test org (`get/post/patch/delete/list/named`) |
| `prefix` | unique name prefix for this test's resources |
| `capture(name, opts)` | annotated screenshot, no-op unless `PNEX_E2E_CAPTURE_DIR` is set |
| `uiLocale`, `t`, `tr` | locale of the project (`en` → en-US, `fr` → fr-FR) and ftl helpers |
| `authenticated` | `test.use({ authenticated: false })` for login flows |

Rules:

- **Labels come from the ftl catalogs**, never hard-coded: `app.t('flows-new')`,
  `app.tr('functions-fix-all')` (exact RegExp, placeables match anything). The
  same test then runs in both locales, and a renamed label breaks no test.
- Prefer roles and accessible names (`getByRole('dialog', { name })`). Dialogs
  (`Modal`, `FormDialog`, `ConfirmDialog`) expose `dialog` / `alertdialog`
  named by their title — use `dialog()` / `confirmDialog()` from `pages/shell.ts`.
- Page objects live in `src/pages/`, one per area; specs stay a readable story.
- A test that needs the French UI as well is tagged `@i18n` (the `fr` project
  only runs those).
- Writing a new step: `ROUTE=/flows ACTIONS="await page.getByRole('button',{name:'New flow'}).click()" bun run explore`
  prints the aria snapshot of the page (and keeps the org untouched).

## Coverage

| Spec | Tag | What it proves |
|---|---|---|
| `smoke` | `@smoke @i18n` | every route renders its title, session kept, no wasm panic — en + fr |
| `auth` | `@auth @i18n` | real Rauthy sign-in (2 steps) and sign-out |
| `functions` | `@functions` | Starlark template, live test, new version, JS runtime error, delete |
| `flows` | `@flows` | canvas build (inject → values → memory write), save, deploy, value in the memory store |
| `dashboards` | `@dashboards` | Value widget bound to a memory key fed by a deployed flow, live view |
| `controls` | `@dashboards` | mobile dashboard (creation modal), switch card declaring its own source at save (no control to pick, D131), a deployed control-source → memory-write flow receives the press; deleting the dashboard deletes the unused source |
| `annotation-controls` | `@studio` | control item on a panorama, operated from the tour preview side panel, reaches a deployed flow |
| `events` | `@events` | Event log node → OpenObserve → Events page |
| `notifications` | `@notifications` | SMTP channel tested from its form: the mail lands in mailcrab |
| `settings` | `@settings` | secrets (value never shown back), organizations create/delete |
| `search` | `@search @i18n` | sidebar search finds a function and deep-links to its editor |
| `map` | `@map` | POI placed by clicking the map, drawer, delete |
| `geo` | `@geo` (`@geo-live`) | no basemap → notice; basemap added from the org page form, Test, default on the map, switcher remembered; with `PNEX_E2E_GEO_BASE`: geocode, reverse and route through the server proxy against Photon + GraphHopper, Test of each provider |
| `media` | `@media` | photo upload (browser-rendered PNG), delete |
| `studio` | `@studio` | virtual tour: floor plan + two labelled 360° scenes, start scene, stored doc, delete |
| `profile-data` | `@profile @mixtures` | language preference switch, CoolProp mixture |
| `firmware-ide` | `@firmware` | starter sketch compiles on the builder, a broken revision reports its error |
| `flow-alert` | `@flows @notifications` | Starlark threshold function → Notification node → email in mailcrab |
| `edge-refs` | `@edge` | Wi-Fi referential, its password stored as a vault secret |
| `a11y` (own project) | `@a11y` | axe-core report per route — `task e2e -- --project=a11y` (`PNEX_E2E_A11Y_STRICT=1` to fail) |
| `hardware` | `@hardware` | real boards, in order: C3 register/build/flash/online, pins, telemetry + quick charts, rebuild (new version) + non-forced OTA, custom firmware (then generic restored), ESP32-CAM register + live view + flash LED, ESP32 DevKit 38p + TFT register (variant + screen) + pins (screen pins reserved, pad readback), NodeMCU V3 + soldered OLED register + pins (D7 readback, A0), Waveshare ESP32-C6-Zero register + pins (GP14 readback, GP0 ADC) + OTA + custom firmware |

## Captures

With `PNEX_E2E_CAPTURE_DIR=<dir>`, every `capture()` call writes
`<dir>/<locale>/<test-slug>/<NN>-<name>.png` and appends one JSON line to
`<dir>/manifest.jsonl`:

```json
{"test":"…","titlePath":["functions","…"],"file":"en-US/…/02-function-editor.png","locale":"en-US",
 "index":2,"name":"function-editor","caption":"Threshold alarm template in Starlark","url":"/functions",
 "tags":["@functions"],"at":"…"}
```

For publishable shots, name things nicely and keep one worker:
`PNEX_E2E_PREFIX=Demo PNEX_E2E_ORG=Demo PNEX_E2E_WORKERS=1` (resources become
"Demo threshold", "Demo tank"…).

Captures hide the AI assistant button and toasts, and blur anything marked
`data-doc-mask` / `.doc-mask` (`maskText(page, secrets)` marks elements by
content). Run with `PNEX_E2E_DPR=2` for crisp images.

The package also exports its building blocks (`package.json` → `exports`),
so another checkout can import the fixtures and page objects and run its
own scenarios against the same stack.

## Hardware (`@hardware`)

Skipped unless the boards are declared:

| Variable | What |
|---|---|
| `PNEX_E2E_C3_PORT` | serial port of an ESP32-C3 (Seeed XIAO profile, e.g. `/dev/ttyACM0`) |
| `PNEX_E2E_CAM_PORT` | serial port of an ESP32-CAM on its MB carrier (e.g. `/dev/ttyUSB0`) |
| `PNEX_E2E_ESP32_PORT` | serial port of an ESP32 DevKit 38 pins (TXD/RXD variant) with its ST7735 TFT wired (SCK 18, MOSI 23, CS 5, DC 2, RST 4) |
| `PNEX_E2E_ESP32U_PORT` | serial port of an ESP32 DevKit 38 pins WROOM-32U (external antenna, CP2102) with the same ST7735 TFT wiring |
| `PNEX_E2E_NODEMCU_PORT` | serial port of a NodeMCU V3 (CH340G) with its soldered 0.96" OLED (SDA D6, SCL D5) |
| `PNEX_E2E_C6_PORT` | serial port of a Waveshare ESP32-C6-Zero (native USB, e.g. `/dev/ttyACM0`) |
| `PNEX_E2E_WIFI_SSID` / `PNEX_E2E_WIFI_PASSWORD` | network the boards join (stored as an Edge referential of the test org) |
| `PNEX_E2E_C3_ID` / `PNEX_E2E_CAM_ID` / `PNEX_E2E_ESP32_ID` / `PNEX_E2E_ESP32U_ID` / `PNEX_E2E_NODEMCU_ID` / `PNEX_E2E_C6_ID` | device ids in the test org (`e2e-c3` / `e2e-cam` / `e2e-esp32` / `e2e-esp32u` / `e2e-nodemcu` / `e2e-c6`) |
| `PNEX_E2E_MAILCRAB_URL` | mailcrab API (`http://localhost:1080`) |

```bash
export PNEX_E2E_C3_PORT=/dev/ttyACM0 PNEX_E2E_CAM_PORT=/dev/ttyUSB0
export PNEX_E2E_WIFI_SSID="<ssid>"
export PNEX_E2E_WIFI_PASSWORD="$(nmcli -s -g 802-11-wireless-security.psk connection show '<ssid>')"
task e2e:hardware
```

Each run registers the boards again through the wizard (the device rows are
deleted first), builds on the server and flashes with `esptool` on the
declared port (Web Serial cannot be driven headless). **Always an explicit
port** — `esptool` without `--port` writes to whatever board it finds
first. The custom-firmware test flashes the C3 with an IDE project, then
puts the generic `e2e-c3` image back.

A live camera films the room it sits in: never publish its captures as
content without checking what they show.

## Linux desktop (`@linux`)

The native desktop app (Dioxus desktop on WebKitGTK) driven through
WebKitWebDriver + WebdriverIO — Playwright cannot attach to WebKitGTK, the
runner stays Playwright. Specs live in `tests-linux/`, config
`playwright.linux.config.ts`, fixtures `src/fixtures-linux.ts` (same `api`,
`t`/`tr`, `prefix` as the web suite; `desktop` is a `DesktopShell` over the
WebDriver session).

```bash
sudo apt install webkit2gtk-driver                  # WebKitWebDriver, once
task e2e:linux                                      # build the e2e app, run everything
task e2e:linux -- tests-linux/lan-scan.spec.ts      # one spec
cd e2e && bunx playwright test -c playwright.linux.config.ts   # app already built
```

| Variable | Default |
|---|---|
| `PNEX_E2E_LINUX_APP` | `target/dx/pnex-frontend/linux-e2e/pnex-frontend` (`task build:frontend:linux:e2e`) |
| `PNEX_E2E_WEBKIT_DRIVER` | `WebKitWebDriver` |

- **The e2e app** has feature `e2e`: `window.__pnexNavigate(path)` (in-memory
  router, `desktop.goto()`) and the vendored wry patch that allows a WebDriver
  session — only when the run also sets `PNEX_E2E_AUTOMATION` (the harness
  does). Release builds (`task build:frontend:linux`) never carry it.
- **Session**: each test gets its own `XDG_DATA_HOME`; the app's
  `pnex/pnex-storage.json` is seeded there before launch
  (`test.use({ session })`: `signed-in`, `server-only`, `fresh`). The
  operator's own `~/.local/share/pnex` is never read nor written.
- **Window**: resized to 1280×800 (dx opens 800 px wide, below `lg`, where
  the map folds its panel).
- **Inputs**: `desktop.fill()` sets the value in one go — WebDriver
  key-by-key typing reaches the Dioxus signal truncated on WebKitGTK.
- **Checks on every test**: no Rust panic printed by the app.

| Spec | Covers |
|---|---|
| `smoke` (`@smoke @i18n`) | signed-in boot, every route renders, en + fr |
| `onboarding` | fresh install: server URL → CA fingerprint (https) → login |
| `lan-scan` | fresh install: LAN scan finds the stack → connect → CA → login |

## Android (`@android`)

The native app on a phone attached through adb, driven through its WebView
(Playwright `_android` + the app's DevTools socket). Specs live in
`tests-android/`, config `playwright.android.config.ts`, fixtures
`src/fixtures-android.ts` (same `api`, `t`/`tr`, `prefix`, `capture` as the
web suite; `app` is an `AndroidShell`).

```bash
task e2e:android                                   # build + install the e2e APK, run everything
task e2e:android -- tests-android/media.spec.ts    # one spec
cd e2e && bunx playwright test -c playwright.android.config.ts   # APK already installed
```

| Variable | Default |
|---|---|
| `PNEX_E2E_ANDROID_API_BASE` | `PNEX_E2E_BASE_URL` — origin **the phone** reaches the stack at (LAN IP, not `localhost`) |
| `PNEX_E2E_ANDROID_SERIAL` | the only device in `adb devices` |
| `PNEX_E2E_ANDROID_INSTALL=1` | install `PNEX_E2E_ANDROID_APK` (default `target/dx/pnex-frontend/pnex-e2e.apk`) before the run |

- **The e2e APK** (`task build:frontend:android:e2e`) is the app with feature
  `e2e`: it adds `window.__pnexNavigate(path)`, the native router keeping an
  in-memory history (`app.goto()` uses it). Never distributed. It does not
  enable `diag`: the suite checks the UI as shipped (no Take 360 debug line).
- **Session**: the app stores it in `files/pnex-storage.json`, written through
  `run-as` before each launch (`test.use({ session })`: `signed-in`,
  `server-only`, `fresh`). The operator's own storage is saved at the start of
  the run and put back at the end.
- **Checks on every test**: no page error, no Rust panic in logcat
  (`PNEX-PANIC` / `panicked at`).
- **Native UI** (camera app, permission dialogs, keys): the suite avoids input
  injection — leaving the camera = relaunching the activity. MIUI refuses
  `input`/`pm grant` unless "USB debugging (Security settings)" is on;
  `canInject` tells a test, which then skips. The CAMERA permission is granted
  once by hand (the media tests skip without it).
- **MIUI installs** ask for confirmation on the phone ("Install via USB"):
  tick "Remember my choice" once.

| Spec | Covers |
|---|---|
| `smoke` (`@smoke @i18n`) | signed-in boot, every route renders, en + fr |
| `onboarding` | fresh install in the OS language → server picker → CA trust (TOFU) → login |
| `media` | Take 360 (guided start, live camera, no debug line, cancel) · photo intent cancelled → no pending upload |
| `deeplink` | `pnex://return` resumes the running app (single instance) |
