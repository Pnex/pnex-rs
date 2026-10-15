// Dev helper (PNEX_E2E_EXPLORE=1): the map of the org loaded by
// _import_schools.spec.ts — France, then zoomed on Paris; times the clusters.
import { test } from '../src/fixtures.ts';

const DIR = process.env.TOUR_DIR ?? 'test-results/schools-map';

test('schools on the map', async ({ app, page }) => {
  const times: string[] = [];
  page.on('response', async (r) => {
    if (r.url().includes('/pois/cluster') || r.url().includes('/pois?')) {
      times.push(`${r.status()} ${r.url().split('/api/v1')[1].slice(0, 90)} ${(await r.body()).length} B`);
    }
  });
  const t0 = Date.now();
  await app.goto('/map');
  await page.waitForTimeout(6000);
  console.log(`map open + 6 s (${Date.now() - t0} ms)`);
  await page.screenshot({ path: `${DIR}/france.png` });
  // No geocoder in a fresh org: zoom with the wheel on the Paris cluster.
  await page.mouse.move(1082, 352);
  for (let i = 0; i < 8; i++) {
    await page.mouse.wheel(0, -400);
    await page.waitForTimeout(400);
  }
  await page.waitForTimeout(4000);
  await page.screenshot({ path: `${DIR}/paris.png` });
  console.log(times.join('\n'));
});
