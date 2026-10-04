// Surfaces on a panorama (D128/D129): an annotation set carries a `control`
// item; the tour preview shows it in the "Controls and readings" panel on the
// right, and operating it reaches a deployed flow (control-source ->
// memory-write) like the dashboard cards do.
import { expect, test } from '../src/fixtures.ts';
import { makePanorama } from '../src/fixtures-files.ts';

test.describe('annotation controls', { tag: '@studio' }, () => {
  test('a control item on a panorama drives a flow', async ({ app, api, page, browser, prefix, capture }) => {
    const pano = await api.upload<{ id: string }>(
      `/media?filename=pano.png&name=${encodeURIComponent(`${prefix} room`)}&kind=panorama`,
      await makePanorama(browser, 'Machine room'),
    );
    const key = `${prefix}.fan`.replace(/[^\w.-]/g, '_').slice(-64);
    const control = await api.post('/controls', {
      key: `${prefix.toLowerCase().replace(/[^a-z0-9]+/g, '_')}.fan`,
      label: `${prefix} fan`,
      spec: { kind: 'switch' },
    });
    const flow = await api.post('/flows', {
      name: `${prefix} fan flow`,
      graph: {
        nodes: [
          {
            id: 'cs',
            kind: 'control_source',
            config: { controls: [control.id] },
            outputs: [{ port: 0, targets: ['mw'] }],
          },
          { id: 'mw', kind: 'memory_write', config: { key, ttl_secs: 3600 } },
        ],
      },
    });
    await api.post(`/flows/${flow.id}/deploy`, {});

    const tourName = `${prefix} room tour`;
    const tour = await api.post('/tours', { name: tourName });
    await api.patch(`/tours/${tour.id}`, {
      expected_version_number: tour.current_version_number ?? 1,
      doc: {
        mode: 'panorama',
        start_scene: 's1',
        floors: [{ id: 'f1', name: 'Ground', level: 0 }],
        scenes: [{ id: 's1', floor_id: 'f1', label: 'Machine room', media_asset_id: pano.id }],
        links: [],
      },
    });
    const layer = await api.post('/annotation-layers', { name: `${prefix} controls`, media_asset_id: pano.id });
    await api.patch(`/annotation-layers/${layer.id}`, {
      expected_version_number: 1,
      doc: {
        items: [
          {
            id: 'c1',
            media_asset_id: pano.id,
            kind: 'control',
            geometry: { type: 'equirect', yaw: 10, pitch: 0 },
            label: 'Fan',
            target: { type: 'control', control_id: control.id },
          },
        ],
      },
    });
    await api.post(`/annotation-layers/${layer.id}/publish`, {});

    try {
      await app.goto('/studio');
      await page.locator('main tr').filter({ hasText: tourName }).getByRole('button', { name: app.t('studio-open') }).click();
      await page.getByRole('main').getByRole('button', { name: app.t('studio-preview') }).click();

      const panel = page.getByText(app.t('annot-surface-title'), { exact: true });
      await expect(panel).toBeVisible({ timeout: 20_000 });
      const toggle = page.getByRole('switch');
      await expect(toggle).toBeEnabled({ timeout: 20_000 });
      await capture('annotation-controls', { caption: 'Control item in the panorama side panel' });
      await toggle.click();
      await expect(toggle).toHaveAttribute('aria-checked', 'true');

      await expect
        .poll(
          async () => {
            const res = await api.post('/memory/values', { refs: [{ key, field: '' }] });
            return res.results[0]?.value ?? null;
          },
          { timeout: 30_000, message: `memory ${key}` },
        )
        .toBe(1);
      const values = await api.post('/controls/values', { ids: [control.id] });
      expect(values.values[control.id].via).toBe(`annotation:${layer.id}`);
    } finally {
      await api.post(`/flows/${flow.id}/stop`, {}).catch(() => {});
      await api.delete(`/flows/${flow.id}`).catch(() => {});
      await api.delete(`/annotation-layers/${layer.id}`).catch(() => {});
      await api.delete(`/tours/${tour.id}`).catch(() => {});
      await api.delete(`/controls/${control.id}`).catch(() => {});
      await api.delete(`/media/${pano.id}`).catch(() => {});
    }
  });
});
