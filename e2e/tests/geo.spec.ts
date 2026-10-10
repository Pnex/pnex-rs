// Geo providers (geo-layers.md phase F): nothing until the org adds a
// basemap, the org page form + Test, the map on the default basemap and its
// switcher; with a live Photon + GraphHopper (PNEX_E2E_GEO_BASE), geocode,
// reverse and route through the server proxy, the address search of the map
// and the address prefilled in a new POI.
import { expect, test } from '../src/fixtures.ts';
import { BASEMAP_DARK_URL, BASEMAP_URL, GEO_BASE } from '../src/env.ts';
import { MapPage } from '../src/pages/map.ts';
import { dialog, fieldAfterLabel } from '../src/pages/shell.ts';

interface Provider {
  id: string;
  name: string;
  kind: string;
}

interface Basemap {
  id: string;
  name: string;
  style_url: string;
  is_default: boolean;
}

// One org-wide default basemap: the tests of this file must not interleave.
test.describe.configure({ mode: 'serial' });

test.describe('geo providers', { tag: '@geo' }, () => {
  test('basemap: notice without provider, added from the org page, switcher', async ({
    app,
    api,
    prefix,
    capture,
  }) => {
    // The suite owns the E2E org: start from no basemap at all.
    for (const p of await api.get<Provider[]>('/geo/providers')) {
      if (p.kind === 'basemap') await api.delete(`/geo/providers/${p.id}`);
    }
    const map = new MapPage(app);
    await map.open();
    await expect(app.page.getByText(app.t('geo-basemap-none'))).toBeVisible();
    await capture('geo-no-basemap', { caption: 'Map without a basemap provider' });

    // Org page: add a basemap through the form, then Test it.
    const light = `${prefix} light`;
    await app.goto('/orgs/current');
    await app.page.getByRole('button', { name: app.t('geo-add'), exact: true }).click();
    await app.page.locator('#geo-providers-name').fill(light);
    await app.page.locator('#geo-providers-kind').selectOption('basemap');
    await app.page.locator('#geo-providers-url').fill(BASEMAP_URL);
    await capture('geo-provider-form', { caption: 'Adding a basemap provider' });
    await app.page.getByRole('button', { name: app.t('geo-save'), exact: true }).click();
    await app.expectToast(app.t('geo-saved'));

    const row = app.page.getByRole('row').filter({ hasText: light });
    await expect(row).toBeVisible();
    await row.getByRole('button', { name: app.t('geo-test'), exact: true }).click();
    await app.expectToast(app.tr('geo-test-ok'));

    const maps = await api.get<Basemap[]>('/geo/basemaps');
    const created = maps.find((m) => m.name === light);
    expect(created?.is_default, 'first basemap becomes the default').toBe(true);
    expect(created?.style_url).toBe(BASEMAP_URL);

    // A second basemap: the map opens on the default, the switcher swaps.
    const dark = `${prefix} dark`;
    await api.post('/geo/providers', {
      name: dark,
      kind: 'basemap',
      capabilities: ['basemap'],
      base_url: BASEMAP_DARK_URL,
    });
    await map.open();
    await expect(app.page.getByText(app.t('geo-basemap-none'))).toHaveCount(0);
    const switcher = app.page.getByRole('combobox', { name: app.t('geo-basemap-label') });
    await expect(switcher).toBeVisible();
    await expect(switcher.locator('option:checked')).toHaveText(light);
    await switcher.selectOption({ label: dark });
    await app.page.waitForTimeout(1500);
    await capture('geo-basemap-switcher', { caption: 'Switching basemaps on the map' });
    // The choice is remembered on this device.
    await map.open();
    await expect(switcher.locator('option:checked')).toHaveText(dark);
    await expect(app.page.getByText(app.t('viz-map-unavailable'))).toHaveCount(0);
  });

  test('geocode, reverse, route, address search and POI prefill', { tag: '@geo-live' }, async ({
    app,
    api,
    prefix,
    capture,
  }) => {
    test.skip(!GEO_BASE, 'PNEX_E2E_GEO_BASE not set (live Photon + GraphHopper)');
    const photon = await api.post<Provider>('/geo/providers', {
      name: `${prefix} photon`,
      kind: 'photon',
      capabilities: ['geocode', 'reverse'],
      default_for: ['geocode', 'reverse'],
      base_url: `${GEO_BASE}/photon`,
      params: { lang: 'fr' },
      rate_limit_per_s: 10,
    });
    const router = await api.post<Provider>('/geo/providers', {
      name: `${prefix} graphhopper`,
      kind: 'graphhopper',
      capabilities: ['route'],
      default_for: ['route'],
      base_url: `${GEO_BASE}/graphhopper`,
      params: { language: 'fr' },
      rate_limit_per_s: 10,
    });

    const hits = await api.get<{ label: string; lat: number; lon: number }[]>(
      '/geo/geocode?q=place%20bellecour%20lyon&limit=2',
    );
    expect(hits[0].label).toContain('Bellecour');
    expect(hits[0].lat).toBeCloseTo(45.757, 2);
    expect(hits[0].lon).toBeCloseTo(4.832, 2);

    const near = await api.get<{ label: string }[]>('/geo/reverse?lat=45.7578&lon=4.8320');
    expect(near[0].label).toContain('Lyon');

    const route = await api.post<{ distance_m: number; duration_s: number; steps: unknown[]; geometry: any }>(
      '/geo/route',
      { points: [[45.7578, 4.832], [45.7606, 4.8597]] },
    );
    expect(route.distance_m).toBeGreaterThan(2_000);
    expect(route.distance_m).toBeLessThan(6_000);
    expect(route.steps.length).toBeGreaterThan(2);
    expect(route.geometry.type).toBe('LineString');

    // The Test button of each provider, from the org page.
    await app.goto('/orgs/current');
    for (const p of [photon, router]) {
      const row = app.page.getByRole('row').filter({ hasText: p.name });
      await row.getByRole('button', { name: app.t('geo-test'), exact: true }).click();
      await app.expectToast(app.tr('geo-test-ok'));
      await app.clearToasts();
    }

    // Map: address search → « + POI » → form prefilled with the address.
    const map = new MapPage(app);
    await map.open();
    const search = app.page.getByRole('searchbox', { name: app.t('geo-search-placeholder') });
    await search.fill('place bellecour lyon');
    await search.press('Enter');
    const addHere = app.page.getByRole('button', { name: app.t('geo-search-add-poi'), exact: true }).first();
    await expect(addHere).toBeVisible();
    await capture('geo-address-search', { caption: 'Address search on the map' });
    await addHere.click();
    const form = dialog(app.page, app.t('poi-add-title'));
    await expect(fieldAfterLabel(form, app.t('poi-field-label'))).toHaveValue(/Bellecour/);
    await expect(fieldAfterLabel(form, app.t('poi-field-location'))).toHaveValue(/Lyon/);
    const label = `${prefix} bellecour`;
    await fieldAfterLabel(form, app.t('poi-field-label')).fill(label);
    await form.getByRole('button', { name: app.t('poi-save'), exact: true }).click();
    await expect(form).toBeHidden();
    const poi = (await api.list<{ id: string; label: string; latitude: number }>('/pois')).find(
      (p) => p.label === label,
    );
    expect(poi?.latitude).toBeCloseTo(45.757, 2);
    await api.delete(`/pois/${poi!.id}`);

    // Add mode: a click on the map prefills the location by reverse geocoding.
    await app.page.getByRole('button', { name: app.t('poi-add') }).click();
    const box = (await map.map.boundingBox())!;
    await app.page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    const created = dialog(app.page, app.t('poi-add-title'));
    await expect(fieldAfterLabel(created, app.t('poi-field-location'))).not.toHaveValue('');
    await created.getByRole('button', { name: app.t('common-close') }).click();
    await expect(created).toBeHidden();
  });
});
