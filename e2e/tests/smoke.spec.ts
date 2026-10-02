// Every route renders its page (title, session kept, no wasm panic, no page
// error), in both UI locales.
import { expect, test } from '../src/fixtures.ts';
import { ROUTES } from '../src/routes.ts';

test.describe('smoke', { tag: ['@smoke', '@i18n'] }, () => {
  for (const route of ROUTES) {
    test(`renders ${route.path}`, async ({ app, page, tr }) => {
      await app.goto(route.path);
      await expect(page.locator('main')).toBeVisible();
      if (route.title) await expect(page.locator('main h1').first()).toHaveText(tr(route.title));
      await expect(app.logout).toBeVisible();
    });
  }
});
