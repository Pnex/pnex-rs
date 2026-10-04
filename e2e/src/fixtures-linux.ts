// Fixtures of the native Linux suite (playwright.linux.config.ts): same
// `api`, `tokens`, `t`/`tr`, `prefix` as the web suite; the app is driven
// through WebDriver (`desktop`), not a Playwright page.
import { storageEntries } from './auth.ts';
import { BASE_URL } from './env.ts';
import { expect, test as base } from './fixtures.ts';
import { t as ftl, tRe } from './i18n.ts';
import { type Browser, LinuxApp } from './linux.ts';

/** Small page API over WebDriver, in the spirit of `AppShell`. */
export class DesktopShell {
  constructor(
    readonly app: LinuxApp,
    readonly locale: string,
  ) {}

  get browser(): Browser {
    return this.app.browser;
  }

  t(key: string): string {
    return ftl(this.locale, key);
  }

  tr(key: string, flags?: string): RegExp {
    return tRe(this.locale, key, flags);
  }

  /** Opens a route through the e2e hook (in-memory router, cf. Android). */
  async goto(route: string): Promise<void> {
    await this.browser.waitUntil(
      () => this.browser.execute(() => typeof (window as any).__pnexNavigate === 'function'),
      { timeout: 30_000, timeoutMsg: 'window.__pnexNavigate missing (not an e2e build, or signed out)' },
    );
    await this.browser.execute((r: string) => (window as any).__pnexNavigate(r), route);
  }

  /**
   * Fills an input in one go: native value setter + an `input` event.
   * WebDriver key-by-key typing reaches the Dioxus signal truncated on
   * WebKitGTK (the field then holds a partial URL).
   */
  async fill(selector: string, value: string): Promise<void> {
    const el = await this.browser.$(selector);
    await el.waitForDisplayed({ timeout: 15_000, timeoutMsg: `${selector} not shown` });
    await this.browser.execute(
      (node: HTMLInputElement, v: string) => {
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
        setter.call(node, v);
        node.dispatchEvent(new Event('input', { bubbles: true }));
        node.dispatchEvent(new Event('change', { bubbles: true }));
      },
      el as unknown as HTMLInputElement,
      value,
    );
  }

  /** Text of the first match of `selector` ('' when absent). */
  async text(selector: string): Promise<string> {
    // One script call: an element handle goes stale when Dioxus re-renders
    // the node between the lookup and the read.
    const text = await this.browser.execute(
      (sel: string) => (document.querySelector(sel) as HTMLElement | null)?.innerText ?? '',
      selector,
    );
    return text.trim();
  }

  /** Waits until `selector`'s text matches `expected`. */
  async expectText(selector: string, expected: string | RegExp, timeout = 15_000): Promise<void> {
    await expect
      .poll(() => this.text(selector), { timeout, message: selector })
      .toMatch(typeof expected === 'string' ? new RegExp(`^${escapeRe(expected)}$`) : expected);
  }

  /** Element whose own text is exactly `text`. */
  byText(text: string, tag = '*') {
    return this.browser.$(`//${tag}[normalize-space()=${xpathString(text)}]`);
  }

  async expectVisible(text: string, tag = '*', timeout = 15_000): Promise<void> {
    await this.byText(text, tag).waitForDisplayed({ timeout, timeoutMsg: `"${text}" not shown` });
  }

  async click(text: string, tag = 'button'): Promise<void> {
    const el = this.byText(text, tag);
    await el.waitForClickable({ timeout: 15_000, timeoutMsg: `${tag} "${text}" not clickable` });
    await el.click();
  }
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** XPath string literal (handles both quote kinds). */
function xpathString(s: string): string {
  if (!s.includes("'")) return `'${s}'`;
  if (!s.includes('"')) return `"${s}"`;
  return `concat('${s.split("'").join(`', "'", '`)}')`;
}

/** What the app finds in its storage when it starts (cf. AndroidSession). */
export type DesktopSession = 'signed-in' | 'server-only' | 'fresh';

type WorkerFixtures = { serverCa: string };

type TestFixtures = {
  session: DesktopSession;
  desktop: DesktopShell;
};

export const test = base.extend<TestFixtures, WorkerFixtures>({
  serverCa: [
    async ({ playwright }, use) => {
      const request = await playwright.request.newContext({ ignoreHTTPSErrors: true });
      const res = await request.get(`${BASE_URL}/api/v1/meta/ca`);
      await use(res.ok() ? await res.text() : '');
      await request.dispose();
    },
    { scope: 'worker' },
  ],

  session: ['signed-in', { option: true }],

  desktop: async ({ session, tokens, runState, uiLocale, serverCa }, use) => {
    const entries: Record<string, string> = {};
    if (session !== 'fresh') {
      entries['pnex.locale'] = uiLocale;
      entries['pnex.api_base'] = BASE_URL;
      if (serverCa) entries['pnex.server_ca'] = serverCa;
    }
    if (session === 'signed-in') Object.assign(entries, storageEntries(tokens, runState.orgId, uiLocale));
    const app = await LinuxApp.launch(entries);
    try {
      await use(new DesktopShell(app, uiLocale));
      expect(app.panics(), 'Rust panics printed by the app').toEqual([]);
    } finally {
      await app.close();
    }
  },
});

export { expect };
