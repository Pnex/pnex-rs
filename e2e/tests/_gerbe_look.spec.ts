// Dev helper: captures the loading mark (gerbe) on a slowed list load and
// next to the logo (runs only with PNEX_E2E_EXPLORE=1).
import { test } from '../src/fixtures.ts';

const DIR = process.env.TOUR_DIR ?? 'test-results/gerbe';

test('gerbe look', async ({ app, page }) => {
  test.setTimeout(120_000);
  await app.goto('/');
  // Every docs list call waits 4 s: the list and the sidebar both load.
  await page.route('**/api/v1/docs**', async (route) => {
    await new Promise((r) => setTimeout(r, 4000));
    await route.continue();
  });
  const knowledge = page.getByRole('link', { name: 'Pages', exact: true }).first();
  if (!(await knowledge.isVisible())) await page.getByRole('button', { name: 'Knowledge' }).first().click();
  await knowledge.click();
  await page.waitForTimeout(600);
  for (let i = 0; i < 5; i++) {
    await page.screenshot({ path: `${DIR}/page-${i}.png` });
    await page.locator('main svg.pnex-gerbe').first().screenshot({ path: `${DIR}/mark-${i}.png` }).catch(() => {});
    await page.locator('aside svg.pnex-gerbe, div svg.pnex-gerbe').first().screenshot({ path: `${DIR}/side-${i}.png` }).catch(() => {});
    await page.waitForTimeout(450);
  }
});
