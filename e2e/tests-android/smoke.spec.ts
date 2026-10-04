// The native app boots signed in and every route renders its page (title,
// no page error, no Rust panic in logcat), in both UI locales.
import { expect, test } from '../src/fixtures-android.ts';
import { ROUTES } from '../src/routes.ts';

test.describe('android smoke', { tag: ['@android', '@smoke', '@i18n'] }, () => {
  test('every route renders', async ({ app, page, tr }) => {
    // Signed in: the mobile header (menu button + logo) replaces the login.
    await expect(page.locator('main')).toBeVisible({ timeout: 30_000 });
    for (const route of ROUTES) {
      await test.step(route.path, async () => {
        await app.goto(route.path);
        await expect(page.locator('main')).toBeVisible();
        if (route.title) await expect(page.locator('main h1').first()).toHaveText(tr(route.title));
      });
    }
  });
});
