// User functions: create from a template, live test, versioning, runtime
// errors, deletion — all through the UI, checked against the API.
import { expect, test } from '../src/fixtures.ts';
import { FunctionsPage } from '../src/pages/functions.ts';

interface FunctionRow {
  id: number;
  name: string;
  language: string;
  current_version_number: number;
}

test.describe('functions', { tag: '@functions' }, () => {
  test('starlark threshold alarm: create, test, new version, delete', async ({ app, api, prefix, capture }) => {
    const name = `${prefix} threshold`;
    const fns = new FunctionsPage(app);
    await fns.open();
    await capture('functions-list', { caption: 'The functions list of the organization' });

    const editor = await fns.create(name, 'starlark', 'threshold');
    // The template declares every port it uses: no quick-fix banner.
    await expect(app.page.getByRole('button', { name: app.tr('functions-fix-all') })).toHaveCount(0);
    await capture('function-editor', { caption: 'Threshold alarm template in Starlark' });

    const created = (await api.named<FunctionRow>('/functions', name))[0];
    expect(created).toMatchObject({ language: 'starlark', current_version_number: 1 });

    const high = await editor.testRun({ value: 25, threshold: 20 });
    expect(high).toEqual({ ok: true, outputs: { alarm: { payload: true } } });
    await capture('function-test-run', { caption: 'Live test: 25 above the threshold raises the alarm' });
    await editor.closeTest();

    const low = await editor.testRun({ value: 5, threshold: 20 });
    expect(low.outputs.alarm).toEqual({ payload: false });
    await editor.closeTest();

    // New version: inclusive comparison.
    const code = (await editor.code.inputValue()).replace('> inputs["threshold"]', '>= inputs["threshold"]');
    await editor.setCode(code);
    await editor.save();
    await expect.poll(async () => (await api.get<FunctionRow>(`/functions/${created.id}`)).current_version_number).toBe(2);

    const edge = await editor.testRun({ value: 20, threshold: 20 });
    expect(edge.outputs.alarm).toEqual({ payload: true });
    await editor.closeTest();

    await fns.open();
    await fns.delete(name);
    expect(await api.named('/functions', name)).toHaveLength(0);
  });

  test('javascript runtime error is reported by the live test', async ({ app, prefix }) => {
    const fns = new FunctionsPage(app);
    await fns.open();
    const editor = await fns.create(`${prefix} broken`, 'js');
    await editor.setCode(
      ['// @input value number "Measured value"', '// @output result number "Result"', 'function handle(inputs, msg) {', '  return { result: inputs.value.nope.deeper };', '}'].join('\n'),
    );
    const res = await editor.testRun({ value: 1 });
    expect(res.ok).toBe(false);
    expect(res.error).toMatch(/^JS execution: TypeError/);
  });
});
