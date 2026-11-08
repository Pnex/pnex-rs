// Rules from the screen (analysis.md D829–D830): created on a type of objects,
// listed with their state, replayed without sending anything, deleted.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('rules', { tag: '@pipelines' }, () => {
  test('a rule is created, replayed and deleted', async ({ app, page, api }) => {
    const tag = Date.now().toString(36);
    const typeKey = `deal${tag}`;
    await api.post('/ontology/types', {
      def: { key: typeKey, name: `Deal ${tag}`, properties: [{ key: 'amount', name: 'Amount', kind: 'number' }] },
    });
    const name = `Big deals ${tag}`;
    const main = page.getByRole('main');
    await app.goto('/rules');
    await main.getByRole('button', { name: app.t('rules-add') }).click();
    const form = dialog(page, app.t('rules-add'));
    await form.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await form.getByLabel(app.t('rules-type')).selectOption({ label: `Deal ${tag}` });
    await form.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(form).toBeHidden();
    const saved = (await api.list<any>('/rules')).find((r) => r.name === name);
    expect(saved.condition.type_key).toBe(typeKey);

    await app.goto('/rules');
    const row = main.locator('tr').filter({ hasText: name });
    await row.getByRole('button', { name: app.t('rules-replay') }).click();
    const replay = dialog(page, new RegExp(name));
    await replay.getByRole('button', { name: app.t('rules-replay'), exact: true }).click();
    await expect(replay.getByTestId('replay-result')).toBeVisible();
    await replay.getByRole('button', { name: /close|fermer/i }).first().click().catch(() => page.keyboard.press('Escape'));

    await row.getByRole('button', { name: app.t('common-delete') }).click();
    await confirmDialog(page, app.t('rules-delete-title')).getByRole('button', { name: app.t('common-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
