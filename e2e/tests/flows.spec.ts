// Flow editor and runtime: build a pipeline in the canvas, save, deploy,
// and check the engine actually ran it (value lands in the memory store).
import { expect, test } from '../src/fixtures.ts';
import { FlowsPage } from '../src/pages/flows.ts';
import { deployMemoryFlow, memoryKey } from '../src/scenarios.ts';

test.describe('flows', { tag: '@flows' }, () => {
  test('inject → json values → memory write runs once deployed', async ({ app, api, prefix, capture }) => {
    const name = `${prefix} memory`;
    const key = memoryKey(prefix, 'boiler');
    const editor = await deployMemoryFlow(app, api, { name, key, field: 'temp', value: 42.5 });
    await capture('flow-memory-deployed', { caption: 'Inject → Json Values → Memory write, deployed' });

    const saved = (await api.named<{ id: number; name: string }>('/flows', name))[0];
    expect(JSON.stringify(await api.get(`/flows/${saved.id}`))).toContain(key);

    await editor.stop();
    await editor.back();
    const flows = new FlowsPage(app);
    await flows.delete(name);
    expect(await api.named('/flows', name)).toHaveLength(0);
  });
});
