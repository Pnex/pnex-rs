// Map: place a POI by clicking the map, open its drawer, delete it.
import { expect, test } from '../src/fixtures.ts';
import { MapPage } from '../src/pages/map.ts';

interface PoiRow {
  id: string;
  label: string;
  icon?: string;
}

test.describe('map', { tag: '@map' }, () => {
  test('POI: placed on the map, listed, deleted', async ({ app, api, prefix, capture }) => {
    const label = `${prefix} boiler room`;
    const map = new MapPage(app);
    await map.open();
    await map.addPoi(label, '🔥');

    await expect(map.panel.getByText(label).first()).toBeVisible();
    const saved = (await api.list<PoiRow>('/pois')).find((p) => p.label === label);
    expect(saved, 'POI persisted').toBeTruthy();
    await map.openPoi(label);
    await capture('map-poi', { caption: 'A POI and its drawer' });

    await map.deleteOpenPoi();
    await expect(map.panel.getByText(label)).toHaveCount(0);
    expect((await api.list<PoiRow>('/pois')).some((p) => p.label === label)).toBe(false);
  });
});
