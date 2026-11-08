// A read-only SQL query from the Pipelines page (analysis.md D825–D828): sent to the
// worker, answered with its rows; a refused query says why. Needs a pipeline worker
// on the stack (`task pipeline-worker:up`, PNEX_E2E_PIPELINE_WORKER=1).
import { expect, test } from '../src/fixtures.ts';

const WORKER = process.env.PNEX_E2E_PIPELINE_WORKER === '1';

test.describe('analysis', { tag: '@pipelines' }, () => {
  test('a query answers with its rows', async ({ app, page, api }) => {
    test.skip(!WORKER, 'needs a pipeline worker (PNEX_E2E_PIPELINE_WORKER=1)');
    const name = `q${Date.now().toString(36)}`;
    const table = await api.post<any>('/datasets', { name });
    await api.upload(`/datasets/${table.id}/transactions/csv`, Buffer.from('name,salary\nAda,5000\nBob,4000\n'), 'text/csv');
    const main = page.getByRole('main');
    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-tab-query') }).click();
    const editor = main.getByLabel(app.t('pipe-query-label'), { exact: true });

    await editor.fill(`SELECT name FROM ${name} WHERE salary = '5000'`);
    await editor.press('Control+Enter');
    const result = main.getByTestId('query-result');
    await expect(result).toContainText('Ada', { timeout: 30_000 });
    await expect(result).not.toContainText('Bob');

    // A table that does not exist: the code, in words.
    await editor.fill('SELECT * FROM no_such_table_here');
    await main.getByRole('button', { name: app.t('pipe-query-run'), exact: true }).click();
    await expect(main.getByText(app.t('err-analysis-table-unknown'))).toBeVisible({ timeout: 30_000 });
    await api.delete(`/datasets/${table.id}`);
  });
});
