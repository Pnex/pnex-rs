// Validation gate (validation-gate.md D833–D836): who decides each write of an internal
// agent, set on its record, and the validator agent it names.
import { expect, test } from '../src/fixtures.ts';

test.describe('agent gate', { tag: '@agents' }, () => {
  test('a write tool goes through a validator, then a person', async ({ app, page, api }) => {
    const tag = Date.now().toString(36);
    const internal = (tools: string[]) => ({ level: 'internal', timeout_secs: 120, llm_calls_max: 10, tools, max_steps: 5 });
    const checker = await api.post('/agents', { name: `checker ${tag}`, role: 'viewer', spec: internal(['list_flows']) });
    const writer = await api.post('/agents', { name: `writer ${tag}`, role: 'member', spec: internal(['list_flows', 'create_flow']) });

    const main = page.getByRole('main');
    await app.goto('/agents');
    const row = main.locator('tr').filter({ hasText: `writer ${tag}` });
    await row.getByRole('button', { name: app.t('agents-edit-open') }).click();
    const mode = main.locator('#agent-gate-create_flow');
    await expect(mode).toHaveValue('human');
    await expect(main.locator('#agent-gate-list_flows')).toHaveCount(0);
    await mode.selectOption('chain');
    await main.locator('#agent-edit-validator').selectOption({ label: `checker ${tag}` });
    await main.getByRole('button', { name: app.t('agents-edit-save') }).click();
    await expect(main.getByText('v2', { exact: true })).toBeVisible();

    const saved = await api.get(`/agents/${writer.id}`);
    expect(saved.spec.gate).toEqual({ create_flow: 'chain' });
    expect(saved.spec.validator_id).toBe(checker.id);
  });
});
