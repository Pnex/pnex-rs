// Edge referentials: a Wi-Fi credential whose password lands in the vault
// (listed as a WiFi usage on the Secrets page), then deleted.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('edge referentials', { tag: '@edge' }, () => {
  test('Wi-Fi credential: added, password in the vault, deleted', async ({ app, page, prefix }) => {
    const ssid = `${prefix}-net`;
    const password = `pw-${prefix}`;
    await app.goto('/edges/refs');
    await page.getByRole('main').getByRole('button', { name: app.t('edgerefs-new-wifi') }).click();
    const dlg = dialog(page, app.t('edgerefs-new-wifi'));
    await dlg.getByRole('textbox', { name: app.t('builds-field-ssid') }).fill(ssid);
    await dlg.getByRole('textbox', { name: app.t('secret-field-type-placeholder') }).fill(password);
    await dlg.getByRole('button', { name: app.t('common-save'), exact: true }).click();
    await expect(dlg).toBeHidden();
    const row = page.locator('main tr').filter({ hasText: ssid });
    await expect(row).toBeVisible();
    await expect(page.getByRole('main')).not.toContainText(password);

    // The password is a vault secret used by this credential.
    await app.goto('/secrets');
    const secret = page.locator('main tr').filter({ hasText: `wifi/${ssid}/password` });
    await expect(secret).toBeVisible();
    await expect(secret).toContainText(app.t('secrets-usage-wifi'));

    await app.goto('/edges/refs');
    await row.getByRole('button', { name: app.t('wizard-ref-delete') }).click();
    await confirmDialog(page, app.t('edgerefs-confirm-delete-title')).getByRole('button', { name: app.t('wizard-ref-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
