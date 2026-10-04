// Phone layout guard (docs/architecture/mobile-ui.md): on every route, no
// sideways page scroll and no action out of reach — list row actions stay on
// screen, the editors' Save / Deploy too. Runs in both UI locales (French
// labels are longer).
import { mobileLayoutIssues } from '../src/mobile-layout.ts';
import { expect, test } from '../src/fixtures-android.ts';
import { ROUTES } from '../src/routes.ts';

test.describe('android layout', { tag: ['@android', '@i18n'] }, () => {
  test('every route fits the phone width', async ({ app, page }) => {
    await expect(page.locator('main')).toBeVisible({ timeout: 30_000 });
    for (const route of ROUTES) {
      await test.step(route.path, async () => {
        await app.goto(route.path);
        await expect(page.locator('main')).toBeVisible();
        expect(await mobileLayoutIssues(page), `layout issues on ${route.path}`).toEqual([]);
      });
    }
  });
});
