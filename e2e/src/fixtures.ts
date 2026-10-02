// Playwright fixtures shared by every spec (and by external runners that
// import this package): an authenticated page in the test org, an API
// client scoped to it, a unique name prefix, and `capture()`.
import { readFileSync } from 'node:fs';
import { test as base, expect } from '@playwright/test';
import { Api } from './api.ts';
import { passwordGrant, storageEntries, type Tokens } from './auth.ts';
import { Capturer, type CaptureOptions } from './capture.ts';
import { PREFIX, STATE_FILE } from './env.ts';
import { t as tr, tRe } from './i18n.ts';
import { AppShell } from './pages/shell.ts';

export interface RunState {
  orgId: number;
  orgName: string;
}

export function readRunState(): RunState {
  try {
    return JSON.parse(readFileSync(STATE_FILE, 'utf8'));
  } catch {
    throw new Error(`missing ${STATE_FILE}: run through playwright test (global setup)`);
  }
}

type WorkerFixtures = {
  runState: RunState;
  tokens: Tokens;
  api: Api;
};

type TestFixtures = {
  /** `false` opens the page without a session (login flows). */
  authenticated: boolean;
  /** UI locale of the page (`pnex.locale`), from the project's `locale`. */
  uiLocale: string;
  /** Unique, sweepable name prefix for resources created by this test. */
  prefix: string;
  app: AppShell;
  capture: (name: string, opts?: CaptureOptions) => Promise<string | undefined>;
  /** Uncaught page errors and wasm panics seen during the test. */
  pageErrors: string[];
  /** ftl message of `key` in the page's locale. */
  t: (key: string) => string;
  /** Exact-match RegExp of `key` in the page's locale (placeables = wildcards). */
  tr: (key: string, flags?: string) => RegExp;
};

export const test = base.extend<TestFixtures, WorkerFixtures>({
  runState: [async ({}, use) => use(readRunState()), { scope: 'worker' }],

  tokens: [
    async ({ playwright }, use) => {
      const request = await playwright.request.newContext({ ignoreHTTPSErrors: true });
      await use(await passwordGrant(request));
      await request.dispose();
    },
    { scope: 'worker' },
  ],

  api: [
    async ({ playwright, tokens, runState }, use) => {
      const request = await playwright.request.newContext({ ignoreHTTPSErrors: true });
      await use(new Api(request, tokens.access, runState.orgId));
      await request.dispose();
    },
    { scope: 'worker' },
  ],

  authenticated: [true, { option: true }],

  uiLocale: async ({ locale }, use) => use(locale ?? 'en-US'),

  t: async ({ uiLocale }, use) => use((key) => tr(uiLocale, key)),

  tr: async ({ uiLocale }, use) => use((key, flags) => tRe(uiLocale, key, flags)),

  // testId is shared by the en and fr projects: the project keeps them apart.
  prefix: async ({}, use, info) => use(`${PREFIX}-${info.project.name}-${info.testId.slice(0, 6)}`),

  pageErrors: async ({ page }, use) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (msg) => {
      // Rust panics in the wasm front surface as console errors.
      if (msg.type() === 'error' && /panicked at/.test(msg.text())) errors.push(msg.text());
    });
    await use(errors);
  },

  page: async ({ page, authenticated, tokens, runState, uiLocale }, use) => {
    if (authenticated) {
      // Set once: later navigations must keep tokens the app refreshed itself.
      await page.addInitScript((entries: Record<string, string>) => {
        for (const [k, v] of Object.entries(entries)) {
          if (localStorage.getItem(k) == null) localStorage.setItem(k, v);
        }
      }, storageEntries(tokens, runState.orgId, uiLocale));
    } else {
      await page.addInitScript((loc: string) => {
        if (localStorage.getItem('pnex.locale') == null) localStorage.setItem('pnex.locale', loc);
      }, uiLocale);
    }
    await use(page);
  },

  app: async ({ page, pageErrors, uiLocale }, use) => {
    await use(new AppShell(page, uiLocale));
    expect(pageErrors, 'page errors / wasm panics').toEqual([]);
  },

  capture: async ({ page, uiLocale }, use, info) => {
    const capturer = new Capturer(page, info, uiLocale);
    await use((name, opts) => capturer.capture(name, opts));
  },
});

export { expect };
