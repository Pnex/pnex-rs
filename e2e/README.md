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

Tags: `@smoke`, `@i18n`, `@functions`, `@flows`, `@hardware` (needs boards,
see below).

## Captures

With `PNEX_E2E_CAPTURE_DIR=<dir>`, every `capture()` call writes
`<dir>/<locale>/<test-slug>/<NN>-<name>.png` and appends one JSON line to
`<dir>/manifest.jsonl`:

```json
{"test":"…","titlePath":["functions","…"],"file":"en-US/…/02-function-editor.png","locale":"en-US",
 "index":2,"name":"function-editor","caption":"Threshold alarm template in Starlark","url":"/functions",
 "tags":["@functions"],"at":"…"}
```

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
| `PNEX_E2E_C3_PORT` | serial port of an ESP32-C3 (e.g. `/dev/ttyACM0`) |
| `PNEX_E2E_CAM_PORT` | serial port of an ESP32-CAM on its MB carrier (e.g. `/dev/ttyUSB0`) |
| `PNEX_E2E_WIFI_SSID` / `PNEX_E2E_WIFI_PASSWORD` | network the boards join (stored as an Edge referential of the test org) |

The device is registered and built through the UI; flashing uses `esptool`
on the declared port (Web Serial cannot be driven headless). **Always an
explicit port** — `esptool` without `--port` writes to whatever board it
finds first.
