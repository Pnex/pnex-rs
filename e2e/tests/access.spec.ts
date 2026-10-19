// Access page (iam.md §9): a custom role edited in the permission grid, a
// team, a service account whose access key is shown once, a role given to
// it — then the key reads with exactly that role.
import { expect, test } from '../src/fixtures.ts';
import { BASE_URL } from '../src/env.ts';
import { dialog, fieldAfterLabel } from '../src/pages/shell.ts';

test.describe('access', { tag: '@access' }, () => {
  test('role, team, service account and its key', async ({ app, page, api, prefix, capture, request }) => {
    const slug = prefix.toLowerCase().replace(/[^a-z0-9-]/g, '-');
    const roleName = `${prefix} flow reader`;
    const main = page.getByRole('main');
    const tab = (key: string) => main.getByRole('tab', { name: app.t(key) });
    await app.goto('/access');
    await expect(main.locator('h1')).toHaveText(app.t('access-title'));

    // Custom role: one click on « View » of Flows allows flows:read.
    await main.getByRole('button', { name: app.t('access-new-role') }).click();
    let dlg = dialog(page, app.t('access-new-role'));
    await fieldAfterLabel(dlg, app.t('access-field-key')).fill(`${slug}-reader`);
    await fieldAfterLabel(dlg, app.t('access-field-name')).fill(roleName);
    await dlg.locator('button[title="flows:read"]').click();
    await expect(dlg.locator('button[title="flows:read"]')).toHaveAttribute(
      'aria-label',
      new RegExp(app.t('access-mark-allow')),
    );
    await capture('access-role-grid', { caption: 'Access: a custom role in the permission grid' });
    await dlg.getByRole('button', { name: app.t('common-save'), exact: true }).click();
    await expect(dlg).toBeHidden();
    await expect(main.locator('tr').filter({ hasText: roleName })).toBeVisible();

    // Team.
    await tab('access-tab-teams').click();
    await main.getByRole('button', { name: app.t('access-new-team') }).click();
    dlg = dialog(page, app.t('access-new-team'));
    await fieldAfterLabel(dlg, app.t('access-field-name')).fill(`${prefix} readers`);
    await dlg.getByRole('button', { name: app.t('common-save'), exact: true }).click();
    await expect(main.locator('tr').filter({ hasText: `${prefix} readers` })).toBeVisible();

    // Service account and its key, shown once.
    await tab('access-tab-service-accounts').click();
    await main.getByRole('button', { name: app.t('access-new-service-account') }).click();
    dlg = dialog(page, app.t('access-new-service-account'));
    const accountName = `${prefix} site app`;
    await fieldAfterLabel(dlg, app.t('access-field-name')).fill(accountName);
    await dlg.getByRole('button', { name: app.t('common-save'), exact: true }).click();
    const accountRow = main.locator('tr').filter({ hasText: accountName });
    await expect(accountRow).toBeVisible();
    await accountRow.getByRole('button', { name: app.t('access-keys') }).click();
    const keys = page.getByRole('dialog');
    await keys.getByRole('button', { name: app.t('access-new-key') }).click();
    const secret = (await keys.locator('code').first().textContent())?.trim() ?? '';
    expect(secret).toMatch(/^pnex_sk_[0-9a-f]{64}$/);
    await capture('access-key-created', { caption: 'Access: a new access key, shown once' });
    await keys.getByRole('button', { name: app.t('common-close') }).click();

    // Give the custom role to the service account.
    await tab('access-tab-assignments').click();
    await main.getByRole('button', { name: app.t('access-new-assignment') }).click();
    dlg = dialog(page, app.t('access-new-assignment'));
    await fieldAfterLabel(dlg, app.t('access-col-holder')).selectOption({ label: accountName });
    await fieldAfterLabel(dlg, app.t('access-col-role')).selectOption({ label: roleName });
    await dlg.getByRole('button', { name: app.t('access-give'), exact: true }).click();
    await expect(main.locator('tr').filter({ hasText: roleName })).toBeVisible();
    const assignments = await api.list<{ role_key: string; service_account_id: string | null }>('/iam/assignments');
    expect(assignments.some((a) => a.role_key === `${slug}-reader` && a.service_account_id)).toBe(true);

    // The key reads flows, and nothing its role does not give.
    const asKey = (path: string) =>
      request.get(`${BASE_URL}/api/v1${path}`, { headers: { Authorization: `Bearer ${secret}` } });
    expect((await asKey('/flows')).status()).toBe(200);
    expect((await asKey('/dashboards')).status()).toBe(403);
    expect((await asKey('/user-info')).status()).toBe(403);
  });
});
