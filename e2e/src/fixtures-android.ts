// Fixtures of the Android suite (playwright.android.config.ts): same `api`,
// `tokens`, `t`/`tr`, `prefix` and `capture` as the web suite, but `page` is
// the WebView of the native app on a device attached through adb.
import type { AndroidDevice } from 'playwright';
import * as android from './android.ts';
import { storageEntries } from './auth.ts';
import { BASE_URL } from './env.ts';
import { expect, test as base } from './fixtures.ts';
import { AppShell } from './pages/shell.ts';

/** Origin the phone reaches the stack at (must be routable from the LAN). */
export const API_BASE = (process.env.PNEX_E2E_ANDROID_API_BASE ?? BASE_URL).replace(/\/+$/, '');

/** App shell over the native WebView: routes open through the e2e hook. */
export class AndroidShell extends AppShell {
  override async goto(route: string): Promise<void> {
    // `window.__pnexNavigate` (feature `e2e`): the native router keeps an
    // in-memory history, a URL navigation would not route.
    await this.page.waitForFunction(() => typeof (window as any).__pnexNavigate === 'function', null, {
      timeout: 30_000,
    });
    await this.page.evaluate((r) => (window as any).__pnexNavigate(r), route);
    await this.settle();
  }
}

/** What the app finds in its storage when it starts. */
export type AndroidSession =
  /** Signed in to the test org (tokens from a password grant). */
  | 'signed-in'
  /** Server chosen and its CA pinned, nobody signed in (login screen). */
  | 'server-only'
  /** Fresh install: no server, no session (server picker). */
  | 'fresh';

type WorkerFixtures = {
  device: AndroidDevice;
  serverCa: string;
};

type TestFixtures = {
  session: AndroidSession;
  /** Native input injection available (taps, keys)? See `android.canInjectInput`. */
  canInject: boolean;
};

export const test = base.extend<TestFixtures, WorkerFixtures>({
  device: [
    async ({}, use) => {
      if (android.INSTALL) android.installApk();
      const device = await android.connectDevice();
      // Often the operator's own phone: put their session back afterwards.
      const saved = android.readStorage();
      await use(device);
      android.writeStorage(saved);
      await device.close();
    },
    { scope: 'worker' },
  ],

  serverCa: [
    async ({ playwright }, use) => {
      const request = await playwright.request.newContext({ ignoreHTTPSErrors: true });
      const res = await request.get(`${BASE_URL}/api/v1/meta/ca`);
      // Plain-http stacks have no CA to pin: the app then skips the TOFU step.
      await use(res.ok() ? await res.text() : '');
      await request.dispose();
    },
    { scope: 'worker' },
  ],

  session: ['signed-in', { option: true }],

  canInject: async ({}, use) => use(android.canInjectInput()),

  page: async ({ device, session, tokens, runState, uiLocale, serverCa }, use) => {
    // A fresh install has nothing stored, not even a locale: the app then
    // follows the OS language.
    const entries: Record<string, string> = {};
    if (session !== 'fresh') {
      entries['pnex.locale'] = uiLocale;
      entries['pnex.api_base'] = API_BASE;
      if (serverCa) entries['pnex.server_ca'] = serverCa;
    }
    if (session === 'signed-in') Object.assign(entries, storageEntries(tokens, runState.orgId, uiLocale));
    android.writeStorage(entries);
    android.clearLogcat();
    const page = await android.openApp(device);
    await use(page);
    expect(android.panics(), 'Rust panics in logcat').toEqual([]);
  },

  app: async ({ page, pageErrors, uiLocale }, use) => {
    await use(new AndroidShell(page, uiLocale));
    expect(pageErrors, 'page errors').toEqual([]);
  },
});

export { expect };
