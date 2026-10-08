// Map POI drawer on desktop and phone: switching between two attached
// dashboards, loading feedback on slow or failing networks.
import { expect, test } from '../src/fixtures.ts';
import type { Api } from '../src/api.ts';
import { MapPage } from '../src/pages/map.ts';

async function seedPoi(api: Api, prefix: string) {
  const poi = await api.post('/pois', { label: `${prefix} plant`, emoji: '📍', latitude: 46.6, longitude: 2.4 });
  const dashboards: { id: string; name: string }[] = [];
  for (const name of [`${prefix} dash A`, `${prefix} dash B`]) {
    const dash = await api.post('/dashboards', { name, layout: { canvas: { width: 1200, height: 800 }, widgets: [] } });
    await api.post('/resources/edges', {
      relation: 'placed_on',
      source_kind: 'map_pin',
      source_id: String(poi.id),
      target_kind: 'dashboard',
      target_id: String(dash.id),
      placement: { label: name },
    });
    dashboards.push({ id: String(dash.id), name });
  }
  return { poi, dashboards };
}

for (const size of [
  { name: 'desktop', viewport: { width: 1600, height: 1000 } },
  { name: 'phone', viewport: { width: 390, height: 844 } },
]) {
  test.describe(`map preview (${size.name})`, { tag: '@map' }, () => {
    test.use({ viewport: size.viewport });

    test('a second dashboard replaces the first in the preview', async ({ app, api, prefix }) => {
      const { poi, dashboards } = await seedPoi(api, prefix);
      const map = new MapPage(app);
      await map.open();
      await map.openPoi(poi.label);

      await map.drawer.getByRole('button', { name: dashboards[0].name }).click();
      await expect(map.preview.getByText(dashboards[0].name).first()).toBeVisible();
      // "Show map" is gone: closing the preview is the way back to the map.
      await expect(map.preview.getByRole('button', { name: app.t('poi-preview-close') })).toBeVisible();

      await map.backToDrawer();
      await map.drawer.getByRole('button', { name: dashboards[1].name }).click();
      await expect(map.preview.getByText(dashboards[1].name).first()).toBeVisible();
      await expect(map.preview.getByText(dashboards[0].name)).toHaveCount(0);
    });
  });
}

test.describe('map on a bad network', { tag: '@map' }, () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('points show a loading state, then a retried failure', async ({ app, api, prefix, page }) => {
    await seedPoi(api, prefix);
    const loading = page.getByRole('status').filter({ hasText: app.t('poi-points-loading') });
    const failed = page.getByRole('status').filter({ hasText: app.t('poi-points-failed') });

    // Slow network: the cluster answer is held, the pill says points load.
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route('**/api/v1/pois/cluster**', async (route) => {
      await held;
      await route.continue();
    });
    const map = new MapPage(app);
    await map.open();
    await expect(loading).toBeVisible();
    release();
    await expect(loading).toBeHidden();
    await expect(page.locator('.maplibregl-marker').first()).toBeVisible();

    // Lost network: the fetch fails, the pill turns amber and retries.
    await page.unroute('**/api/v1/pois/cluster**');
    await page.route('**/api/v1/pois/cluster**', (route) => route.abort());
    await map.ensurePanel();
    await page.getByRole('searchbox').fill(prefix);
    await expect(failed).toBeVisible();
    await page.unroute('**/api/v1/pois/cluster**');
    await expect(failed).toBeHidden({ timeout: 15_000 });
  });

  test('a dashboard that fails to load offers a retry', async ({ app, api, prefix, page }) => {
    const { poi, dashboards } = await seedPoi(api, prefix);
    const map = new MapPage(app);
    await map.open();
    await map.openPoi(poi.label);

    const detail = `**/api/v1/dashboards/${dashboards[0].id}`;
    await page.route(detail, (route) => route.abort());
    await map.drawer.getByRole('button', { name: dashboards[0].name }).click();
    const retry = map.preview.getByRole('button', { name: app.t('common-retry') });
    await expect(retry).toBeVisible();
    await expect(map.preview.getByText(app.t('dashboard-live-unavailable'))).toBeVisible();

    await page.unroute(detail);
    await retry.click();
    await expect(retry).toBeHidden();
  });
});
