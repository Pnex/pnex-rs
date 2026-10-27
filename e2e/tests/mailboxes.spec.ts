// Mailboxes: one added with its password in the vault, tested against a
// server that does not exist (the test says why), paused, resumed, deleted.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('mailboxes', { tag: '@mail' }, () => {
  test('mailbox: added, tested, paused, deleted', async ({ app, page, prefix }) => {
    const name = `${prefix} sales`;
    const password = `pw-${prefix}`;
    await app.goto('/mailboxes');
    await page.getByRole('main').getByRole('button', { name: app.t('mailboxes-new') }).click();
    const dlg = dialog(page, app.t('mailboxes-new'));
    await dlg.getByLabel(app.t('mailboxes-field-name'), { exact: true }).fill(name);
    await dlg.getByLabel(app.t('mailboxes-field-address'), { exact: true }).fill(`${prefix}@example.invalid`);
    await dlg.getByLabel(app.t('mailboxes-field-host'), { exact: true }).fill('imap.example.invalid');
    await dlg.getByLabel(app.t('mailboxes-field-user'), { exact: true }).fill(`${prefix}@example.invalid`);
    await dlg.getByRole('textbox', { name: app.t('mailboxes-field-password') }).fill(password);
    await dlg.getByRole('button', { name: app.t('common-add'), exact: true }).click();
    await expect(dlg).toBeHidden();

    const row = page.locator('main tr').filter({ hasText: name });
    await expect(row).toBeVisible();
    await expect(row).toContainText(app.t('mailboxes-owner-me'));
    await expect(page.getByRole('main')).not.toContainText(password);

    // The server does not resolve: the test fails and says so, nothing is stored.
    await row.getByRole('button', { name: app.t('mailboxes-test') }).click();
    const result = dialog(page, app.tr('mailboxes-test-title'));
    await expect(result).toContainText(app.t('mailboxes-test-failed'));
    await result.getByRole('button', { name: app.t('common-close') }).last().click();

    await row.getByRole('button', { name: app.t('mailboxes-pause') }).click();
    await expect(row).toContainText(app.t('mailboxes-state-paused'));
    await row.getByRole('button', { name: app.t('mailboxes-resume') }).click();
    await expect(row).toContainText(app.t('mailboxes-state-active'));

    // The password is a vault secret used by this mailbox.
    await app.goto('/secrets');
    await expect(page.locator('main tr').filter({ hasText: app.t('secrets-usage-mailbox') }).filter({ hasText: name })).toBeVisible();

    await app.goto('/mailboxes');
    await row.getByRole('button', { name: app.t('common-delete') }).click();
    await confirmDialog(page, app.t('mailboxes-confirm-delete-title')).getByRole('button', { name: app.t('common-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
