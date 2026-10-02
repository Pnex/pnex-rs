// Organization settings: secrets vault (write-only values) and the
// organizations list (create, manage, delete).
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog, fieldAfterLabel } from '../src/pages/shell.ts';

test.describe('settings', { tag: '@settings' }, () => {
  test('secret: created, value never shown back, deleted', async ({ app, page, api, prefix, capture }) => {
    const name = `${prefix}-token`;
    const value = `s3cr3t-${prefix}`;
    await app.goto('/secrets');
    await page.getByRole('main').getByRole('button', { name: app.t('secrets-new') }).click();
    const dlg = dialog(page, app.t('secrets-new-title'));
    await fieldAfterLabel(dlg, app.t('secrets-name')).fill(name);
    await fieldAfterLabel(dlg, app.t('secrets-description')).fill('E2E secret');
    await fieldAfterLabel(dlg, app.t('secrets-value')).fill(value);
    await dlg.getByRole('button', { name: app.t('secrets-save'), exact: true }).click();
    await expect(dlg).toBeHidden();

    const row = page.locator('main tr').filter({ hasText: name });
    await expect(row).toBeVisible();
    await expect(page.getByRole('main')).not.toContainText(value);
    const listed = (await api.list<{ name: string }>('/secrets')).find((s) => s.name === name);
    expect(JSON.stringify(listed)).not.toContain(value);
    await capture('secrets-list', { caption: 'Secrets: names and usages, never values' });

    // Two-step inline delete.
    await row.getByRole('button', { name: app.t('secrets-delete') }).click();
    await row.getByRole('button', { name: app.t('secrets-confirm-delete') }).click();
    await expect(row).toHaveCount(0);
  });

  test('organization: create, manage, delete', async ({ app, page, api, prefix }) => {
    const name = `${prefix} org`;
    await app.goto('/orgs');
    await page.getByRole('textbox', { name: app.t('orgs-new-placeholder') }).fill(name);
    await page.getByRole('main').getByRole('button', { name: app.t('orgs-create'), exact: true }).click();
    const row = page.locator('main tr').filter({ hasText: name });
    await expect(row).toBeVisible();

    await row.getByRole('button', { name: app.t('orgs-manage') }).click();
    await expect(page.getByRole('main').getByRole('heading', { name, level: 2 })).toBeVisible();
    await page.getByRole('main').getByRole('button', { name: app.t('orgs-delete') }).click();
    await confirmDialog(page, app.t('orgs-confirm-delete-title')).getByRole('button', { name: app.t('orgs-delete') }).click();
    // Back on the list once the delete is done.
    await expect(page.locator('main h1')).toHaveText(app.t('orgs-title'));
    await expect(page.locator('main tr').filter({ hasText: name })).toHaveCount(0);
    await expect.poll(async () => (await api.list<{ name: string }>('/orgs')).some((o) => o.name === name)).toBe(false);
  });
});
