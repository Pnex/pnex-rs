// Dev helper (PNEX_E2E_EXPLORE=1): slows the API down and captures the
// loading gerbe behind the sidebar logo, light then dark (D224).
import { test } from '../src/fixtures.ts';

const DIR = process.env.TOUR_DIR ?? 'test-results/header-gerbe';

test('header gerbe while loading', async ({ app, page }) => {
  await app.goto('/cases');
  await page.waitForTimeout(1500);
  await page.route('**/api/v1/**', async (route) => {
    await new Promise((r) => setTimeout(r, 3000));
    await route.continue();
  });
  for (const theme of ['light', 'dark']) {
    await page.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme);
    await page.getByRole('button', { name: /refresh|rafra/i }).first().click();
    await page.waitForTimeout(1200);
    const state = await page.locator('[data-pnex-gerbe]').first().getAttribute('data-pnex-gerbe');
    console.log(`${theme}: data-pnex-gerbe=${state}`);
    await page.locator('[data-pnex-gerbe]').first().locator('..').screenshot({ path: `${DIR}/${theme}.png` });
    await page.waitForTimeout(3000);
  }
});
