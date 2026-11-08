// Markings and clearances from the screen (governance.md D798–D806): an admin
// creates a marking, marks a table and is refused its rows until cleared for a
// purpose, revokes, declassifies with a reason, and exports an object.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('governance', { tag: '@pipelines' }, () => {
  test('a marked table reads only with a clearance', async ({ app, page, api, prefix }) => {
    const key = `m${Date.now().toString(36)}`;
    const name = `${prefix} HR ${key}`;
    const main = page.getByRole('main');

    await app.goto('/access?tab=governance');
    await main.getByRole('button', { name: app.t('gov-new-marking') }).click();
    const form = dialog(page, app.t('gov-new-marking'));
    await form.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await form.getByLabel(app.t('gov-col-key'), { exact: true }).fill(key);
    await form.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(form).toBeHidden();
    // The list pages by 10: checked through the API, wherever it sorts.
    expect((await api.list<any>('/governance/markings')).some((m) => m.key === key)).toBe(true);

    // A table with rows, marked from its page: its rows are refused at once.
    const table = await api.post<any>('/datasets', { name: `${key}_staff` });
    await api.upload(`/datasets/${table.id}/transactions/csv`, Buffer.from('name,salary\nAda,5000\nBob,4000\n'), 'text/csv');
    await app.goto(`/pipelines/tables/${table.id}`);
    const panel = main.getByTestId('markings');
    await panel.getByRole('combobox').selectOption({ label: name });
    await panel.getByRole('button', { name: app.t('gov-mark') }).click();
    await expect(panel).toContainText(key);
    await expect(main.getByText(app.t('err-marking-required').slice(0, 30))).toBeVisible();
    await expect(main.getByTestId('table-sample')).toHaveCount(0);

    // Cleared for a purpose: the rows show.
    await app.goto('/access?tab=governance');
    await main.getByRole('button', { name: app.t('gov-grant') }).click();
    const grant = dialog(page, app.t('gov-grant'));
    await grant.getByLabel(app.t('gov-col-member')).selectOption({ index: 1 });
    await grant.getByLabel(app.t('gov-col-marking')).selectOption({ label: name });
    await grant.getByLabel(app.t('gov-col-purpose')).fill(`payroll ${key}`);
    await grant.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(grant).toBeHidden();
    const row = main.locator('tr').filter({ hasText: `payroll ${key}` });
    await expect(row).toContainText(app.t('gov-state-active'));
    await app.goto(`/pipelines/tables/${table.id}`);
    await expect(main.getByTestId('table-sample')).toContainText('Ada');

    // Revoked: refused again.
    await app.goto('/access?tab=governance');
    await row.getByRole('button', { name: app.t('gov-revoke') }).click();
    await confirmDialog(page, app.t('gov-revoke-title')).getByRole('button', { name: app.t('gov-revoke') }).click();
    // Active ones list first: the revoked one leaves the page, checked through the API.
    await expect(row).toHaveCount(0);
    const mine = (await api.list<any>('/governance/clearances')).find((c) => c.purpose === `payroll ${key}`);
    expect(mine.active).toBe(false);

    // Declassified with a reason: readable by all again.
    await app.goto(`/pipelines/tables/${table.id}`);
    await panel.getByRole('button', { name: app.t('gov-declassify') }).click();
    const reason = dialog(page, new RegExp(app.t('gov-declassify')));
    const submit = reason.getByRole('button', { name: app.t('gov-declassify'), exact: true });
    await reason.getByLabel(app.t('gov-justification')).fill('too short');
    await expect(submit).toBeDisabled();
    await reason.getByLabel(app.t('gov-justification')).fill('salaries replaced by bands');
    await submit.click();
    await expect(panel).toContainText(app.t('gov-none').slice(0, 20));
    await expect(main.getByTestId('table-sample')).toContainText('Ada');
    await api.delete(`/datasets/${table.id}`);
  });

  test('an object exports as one file', async ({ app, page, api, prefix }) => {
    const typeKey = `t${Date.now().toString(36)}`;
    await api.post('/ontology/types', { def: { key: typeKey, name: `${prefix} client`, properties: [] } });
    const obj = await api.post<any>('/ontology/objects', {
      type_key: typeKey,
      title: `${prefix} export`,
      properties: {},
    });
    await app.goto(`/ontology/object/${obj.id}`);
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.getByTestId('object-export').click(),
    ]);
    expect(download.suggestedFilename()).toBe(`export-${obj.id}.json`);
    await expect(page.getByRole('main').getByTestId('markings')).toBeVisible();
  });
});
