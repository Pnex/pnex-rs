// Surfaces (D123-D131): a switch card added to a mobile dashboard declares
// its own control, provisioned when the dashboard is saved (no control to
// pick first). A deployed flow (control-source -> memory-write) listening to
// that source receives it and writes the memory store. The whole chain runs
// through the real runtime: "a surface reads, a flow acts".
import { expect, test } from '../src/fixtures.ts';
import { DashboardsPage } from '../src/pages/dashboards.ts';
import { memoryKey } from '../src/scenarios.ts';

test.describe('surface controls', { tag: '@dashboards' }, () => {
  test('mobile switch declares a source that drives a flow', async ({ app, api, prefix, capture }) => {
    const key = memoryKey(prefix, 'light');
    const name = `${prefix} phone`;
    const dashboards = new DashboardsPage(app);
    await dashboards.open();
    const editor = await dashboards.create(name, 'mobile');
    await editor.addWidget(app.t('lib-kind-switch'));
    // Before the save: the reference is shown, nothing to pick.
    await expect(editor.inspector.getByText(app.t('insp-source-pending'))).toBeVisible();
    await editor.save();
    await capture('mobile-composer', { caption: 'Mobile composer with a switch card' });

    // The save provisioned the source, listed under this dashboard.
    const found = await api.get(`/dashboards?search=${encodeURIComponent(name)}`);
    const dashboard = await api.get(`/dashboards/${found.results[0].id}`);
    const switchWidget = dashboard.layout.widgets.find((w: any) => w.type === 'switch');
    const controlId: string = switchWidget.options.control.control_id;
    const control = await api.get(`/controls/${controlId}`);
    expect(control.origin.surface).toBe('dashboard');
    expect(control.origin.surface_name).toBe(name);
    expect(control.origin.item_id).toBe(switchWidget.id);

    const flow = await api.post('/flows', {
      name: `${prefix} light flow`,
      graph: {
        nodes: [
          {
            id: 'cs',
            kind: 'control_source',
            config: { controls: [controlId], emit_on_start: true },
            outputs: [{ port: 0, targets: ['mw'] }],
          },
          { id: 'mw', kind: 'memory_write', config: { key, ttl_secs: 3600 } },
        ],
      },
    });
    await api.post(`/flows/${flow.id}/deploy`, {});

    try {
      await editor.back();

      // Live: the control is listened to by the deployed flow (no "no effect").
      const main = app.page.getByRole('main');
      const toggle = main.getByRole('switch');
      await expect(toggle).toBeEnabled({ timeout: 20_000 });
      await expect(main.getByText(app.t('controls-idle'), { exact: true })).toHaveCount(0, {
        timeout: 20_000,
      });
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
      const values = await api.post('/controls/values', { ids: [controlId] });
      expect(values.values[controlId].v).toBe(1);
      expect(values.values[controlId].via).toMatch(/^dashboard:/);
      await capture('mobile-live', { caption: 'Switch operated, flow wrote the memory' });
    } finally {
      await api.post(`/flows/${flow.id}/stop`, {}).catch(() => {});
      await api.delete(`/flows/${flow.id}`).catch(() => {});
    }
    // The flow is gone: deleting the dashboard deletes its unused source.
    await dashboards.backToList();
    await dashboards.delete(name);
    await expect
      .poll(async () => (await api.get(`/controls?search=${encodeURIComponent(control.key)}`)).count)
      .toBe(0);
  });
});
