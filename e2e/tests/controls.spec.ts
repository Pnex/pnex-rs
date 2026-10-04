// Surfaces (D123-D129): a switch card on a mobile dashboard writes an org
// control; a deployed flow (control-source -> memory-write) receives it and
// writes the memory store. The whole chain runs through the real runtime:
// "a surface reads, a flow acts".
import { expect, test } from '../src/fixtures.ts';
import { DashboardsPage } from '../src/pages/dashboards.ts';
import { memoryKey } from '../src/scenarios.ts';

test.describe('surface controls', { tag: '@dashboards' }, () => {
  test('mobile switch drives a flow through an org control', async ({ app, api, prefix, capture }) => {
    const key = memoryKey(prefix, 'light');
    const controlKey = `${prefix.toLowerCase().replace(/[^a-z0-9]+/g, '_')}.light`;
    const control = await api.post('/controls', {
      key: controlKey,
      label: `${prefix} light`,
      spec: { kind: 'switch' },
    });
    const flow = await api.post('/flows', {
      name: `${prefix} light flow`,
      graph: {
        nodes: [
          {
            id: 'cs',
            kind: 'control_source',
            config: { controls: [control.id], emit_on_start: true },
            outputs: [{ port: 0, targets: ['mw'] }],
          },
          { id: 'mw', kind: 'memory_write', config: { key, ttl_secs: 3600 } },
        ],
      },
    });
    await api.post(`/flows/${flow.id}/deploy`, {});

    try {
      const name = `${prefix} phone`;
      const dashboards = new DashboardsPage(app);
      await dashboards.open();
      const editor = await dashboards.create(name, 'mobile');
      await editor.addWidget(app.t('lib-kind-switch'));
      await editor.field('insp-control').selectOption({ value: control.id });
      await editor.save();
      await capture('mobile-composer', { caption: 'Mobile composer with a switch card' });
      await editor.back();

      // Live: the control is listened to by the deployed flow (no "no effect").
      const main = app.page.getByRole('main');
      const toggle = main.getByRole('switch');
      await expect(toggle).toBeEnabled({ timeout: 20_000 });
      await expect(main.getByText(app.t('controls-idle'), { exact: true })).toHaveCount(0);
      await toggle.click();
      await expect(toggle).toHaveAttribute('aria-checked', 'true');

      // The flow received it and wrote the memory key.
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
      expect(values.values[control.id].v).toBe(1);
      expect(values.values[control.id].via).toMatch(/^dashboard:/);
      await capture('mobile-live', { caption: 'Switch operated, flow wrote the memory' });

      await dashboards.backToList();
      await dashboards.delete(name);
    } finally {
      await api.post(`/flows/${flow.id}/stop`, {}).catch(() => {});
      await api.delete(`/flows/${flow.id}`).catch(() => {});
      await api.delete(`/controls/${control.id}`).catch(() => {});
    }
  });
});
