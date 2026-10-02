// Application chrome: routing, sidebar, toasts, global search.
import { expect, type Locator, type Page } from '@playwright/test';
import { t, tRe } from '../i18n.ts';

/** Toast stack (top-right); toasts intercept clicks while visible. */
export const TOASTS = '.fixed.top-4.right-4.z-50';

export class AppShell {
  constructor(
    readonly page: Page,
    /** UI locale of the page ("en-US" / "fr-FR"). */
    readonly locale: string,
  ) {}

  /** ftl message of `key` in the page's locale. */
  t(key: string): string {
    return t(this.locale, key);
  }

  /** Exact-match RegExp of `key` (placeables match anything). */
  tr(key: string, flags?: string): RegExp {
    return tRe(this.locale, key, flags);
  }

  /** Opens an app route and waits for the wasm front to settle. */
  async goto(route: string): Promise<void> {
    await this.page.goto(route, { waitUntil: 'domcontentloaded' });
    await this.settle();
  }

  async settle(): Promise<void> {
    await this.page.waitForLoadState('networkidle', { timeout: 15_000 }).catch(() => {});
  }

  /** Left navigation (the right drawer is also an <aside>). */
  get sidebar(): Locator {
    return this.page.locator('aside').first();
  }

  /** Sign-out button of the sidebar footer (present only with a session). */
  get logout(): Locator {
    return this.page.locator(`button[title="${this.t('shell-logout')}"]`);
  }

  toasts(): Locator {
    return this.page.locator(`${TOASTS} > *`);
  }

  /** Waits for a toast matching `text` (success or error). */
  async expectToast(text: string | RegExp): Promise<void> {
    await expect(this.toasts().filter({ hasText: text }).first()).toBeVisible();
  }

  /** Waits until no toast is left (keeps them out of clicks and shots). */
  async clearToasts(): Promise<void> {
    await this.toasts().first().waitFor({ state: 'detached', timeout: 10_000 }).catch(() => {});
  }
}

/** Modal dialog titled by the ftl message `title` (FormDialog / Modal). */
export function dialog(page: Page, title: string | RegExp): Locator {
  return page.getByRole('dialog', { name: title });
}

/** Confirmation dialog (ConfirmDialog) titled `title`. */
export function confirmDialog(page: Page, title: string | RegExp): Locator {
  return page.getByRole('alertdialog', { name: title });
}
