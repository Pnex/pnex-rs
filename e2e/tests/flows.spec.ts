// Flow editor and runtime: build a pipeline in the canvas, save, deploy,
// and check the engine actually ran it (value lands in the memory store).
import { expect, test } from '../src/fixtures.ts';
import { FlowsPage } from '../src/pages/flows.ts';

interface FlowRow {
  id: number;
  name: string;
  status: string;
  latest_version_number: number;
  deployed_version_number: number | null;
}

test.describe('flows', { tag: '@flows' }, () => {
  test('inject → json values → memory write runs once deployed', async ({ app, api, prefix, capture }) => {
    const name = `${prefix} memory`;
    const key = `${prefix.replace(/[^\w.-]/g, '_')}.temp`;
    const flows = new FlowsPage(app);
    await flows.open();
    const editor = await flows.create(name);

    // The new flow starts with an Inject node: fire every 2 s.
    await editor.select(app.t('flows-palette-inject'));
    await editor.field(app.t('flows-inject-repeat')).fill('2');

    await editor.addNode('flows-palette-value');
    await editor.inspector.getByRole('textbox', { name: 'key', exact: true }).fill('temp');
    await editor.inspector.getByRole('textbox', { name: 'value', exact: true }).fill('42.5');
    await editor.move(app.t('flows-palette-value'), 320, 120);

    await editor.addNode('flows-palette-memory-write');
    await editor.inspector.getByRole('textbox', { name: new RegExp(`^${app.t('flows-memory-key')}\b`) }).fill(key);
    await editor.move(app.t('flows-palette-memory-write'), 600, 120);
    await editor.closeInspector();

    await editor.wire(app.t('flows-palette-inject'), 0, app.t('flows-palette-value'));
    await editor.wire(app.t('flows-palette-value'), 0, app.t('flows-palette-memory-write'));
    await capture('flow-memory-graph', { caption: 'Inject → Json Values → Memory write' });

    await editor.save();
    const row = (await api.named<FlowRow>('/flows', name))[0];
    const graph = await api.get(`/flows/${row.id}`);
    expect(JSON.stringify(graph)).toContain(key);

    await editor.deploy();
    await expect
      .poll(
        async () => {
          const res = await api.post('/memory/values', { refs: [{ key, field: 'temp' }] });
          return res.results[0]?.value ?? null;
        },
        { timeout: 30_000 },
      )
      .toBe(42.5);
    await capture('flow-memory-deployed', { caption: 'Deployed flow' });

    await editor.stop();
    await editor.back();
    await flows.delete(name);
  });
});
