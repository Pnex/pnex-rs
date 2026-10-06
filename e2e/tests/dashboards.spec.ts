// Dashboards: widgets bound to a memory key and to a telemetry series
// written by live flows.
import { expect, test } from '../src/fixtures.ts';
import { DashboardsPage } from '../src/pages/dashboards.ts';
import { FlowsPage } from '../src/pages/flows.ts';
import { deployMemoryFlow, deployMetricFlow, memoryKey } from '../src/scenarios.ts';

test.describe('dashboards', { tag: '@dashboards' }, () => {
  test('value widget shows a live memory value from a flow', async ({ app, api, prefix, capture }) => {
    const key = memoryKey(prefix, 'tank');
    const flowName = `${prefix} level feed`;
    const flow = await deployMemoryFlow(app, api, { name: flowName, key, field: 'level', value: 73.4 });

    const name = `${prefix} tank`;
    const dashboards = new DashboardsPage(app);
    await dashboards.open();
    const editor = await dashboards.create(name);
    await editor.addWidget(app.t('lib-kind-stat'));
    await editor.field('insp-widget-title').fill('Tank level');
    await editor.field('insp-source').selectOption({ value: `mem:${key}` });
    await editor.field('insp-unit').fill('%');
    await editor.save();
    await capture('dashboard-value-widget', { caption: 'Value widget bound to a memory key' });

    // Leaving edit mode lands on the live view.
    await editor.back();
    await expect(app.page.getByRole('main').getByText('73.4')).toBeVisible({ timeout: 20_000 });
    await capture('dashboard-live', { caption: 'Live dashboard' });

    await dashboards.backToList();
    await dashboards.view(name);
    await expect(app.page.getByRole('main').getByText('73.4')).toBeVisible({ timeout: 20_000 });
    await dashboards.backToList();
    await dashboards.delete(name);
    await flow.app.goto('/flows');
    const flows = new FlowsPage(app);
    const ed = await flows.openFlow(flowName);
    await ed.stop();
    await ed.back();
    await flows.delete(flowName);
  });

  test('thermo-hygro card and value widget read a telemetry series', async ({ app, api, prefix, capture }) => {
    const flowName = `${prefix} room climate`;
    const { flow, device, series } = await deployMetricFlow(app, api, {
      name: flowName,
      metric: `${prefix}_room`.replace(/[^\w]/g, '_').toLowerCase(),
      fields: { temperature: 21.5, humidity: 48 },
    });

    // Mobile: a thermo-hygro card bound to both series.
    const dashboards = new DashboardsPage(app);
    await dashboards.open();
    const name = `${prefix} bedroom`;
    const editor = await dashboards.create(name, 'mobile');
    await editor.addWidget(app.t('hcard-thermo_hygro'));
    for (const role of ['temperature', 'humidity'] as const) {
      const value = `t|${device}|${series[role]}`;
      // Role label, then its source select.
      const select = editor.inspector
        .getByText(app.t(`hrole-${role}`), { exact: true })
        .first()
        .locator('xpath=(following::select)[1]');
      await select.selectOption({ value });
    }
    await editor.save();
    await editor.back();
    await expect(app.page.getByRole('main').getByText('21.5')).toBeVisible({ timeout: 30_000 });
    await expect(app.page.getByRole('main').getByText(app.tr('hcard-humidity'))).toBeVisible();
    await capture('dashboard-thermo-hygro', { caption: 'Thermo-hygro card fed by a telemetry series' });
    await dashboards.backToList();
    await dashboards.delete(name);

    // Desktop: a Value widget on the temperature series.
    await dashboards.open();
    const desk = await dashboards.create(`${prefix} desk`);
    await desk.addWidget(app.t('lib-kind-stat'));
    await desk.field('insp-source').selectOption({ value: device });
    await desk.field('insp-metric').selectOption({ value: series.temperature });
    await desk.save();
    await desk.back();
    await expect(app.page.getByRole('main').getByText('21.5')).toBeVisible({ timeout: 30_000 });
    await dashboards.backToList();
    await dashboards.delete(`${prefix} desk`);

    await flow.app.goto('/flows');
    const flows = new FlowsPage(app);
    const ed = await flows.openFlow(flowName);
    await ed.stop();
    await ed.back();
    await flows.delete(flowName);
  });
});
