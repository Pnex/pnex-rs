// Dashboards: a Value widget bound to a memory key written by a live flow.
import { expect, test } from '../src/fixtures.ts';
import { DashboardsPage } from '../src/pages/dashboards.ts';
import { FlowsPage } from '../src/pages/flows.ts';
import { deployMemoryFlow, memoryKey } from '../src/scenarios.ts';

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

    await editor.back();
    await dashboards.view(name);
    await expect(app.page.getByRole('main').getByText('73.4')).toBeVisible({ timeout: 20_000 });
    await capture('dashboard-live', { caption: 'Live dashboard' });

    await dashboards.open();
    await dashboards.delete(name);
    await flow.app.goto('/flows');
    const flows = new FlowsPage(app);
    const ed = await flows.openFlow(flowName);
    await ed.stop();
    await ed.back();
    await flows.delete(flowName);
  });
});
