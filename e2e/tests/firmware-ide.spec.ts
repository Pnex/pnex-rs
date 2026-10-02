// Custom firmware IDE: the starter sketch compiles on the builder; a
// broken revision reports its compile error.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('firmware IDE', { tag: '@firmware' }, () => {
  test('ESP32-C3 starter compiles; a broken revision does not', async ({ app, page, prefix, capture }) => {
    test.setTimeout(20 * 60_000);
    const name = `${prefix} blink`;
    await app.goto('/firmware');
    await page.getByRole('main').getByRole('button', { name: app.t('firmware-new') }).click();
    const dlg = dialog(page, app.t('firmware-create-title'));
    await dlg.getByRole('textbox', { name: app.t('firmware-field-name') }).fill(name);
    await dlg.getByRole('combobox', { name: new RegExp(`^${app.t('firmware-field-chip')}`) }).selectOption({ label: 'ESP32-C3' });
    await dlg.getByRole('button', { name: app.t('firmware-new'), exact: true }).click();
    await expect(dlg).toBeHidden();

    const main = page.getByRole('main');
    const code = main.locator('textarea').first();
    await expect(code).toHaveValue(/PnexDevice pnex;/);

    // Starter revision r1 compiles (first build may fetch the toolchain).
    await main.getByRole('button', { name: app.t('firmware-verify'), exact: true }).click();
    await expect(main.getByText(app.tr('firmware-check-ok'))).toBeVisible({ timeout: 15 * 60_000 });
    await capture('firmware-verify-ok', { caption: 'Starter sketch verified on the builder' });

    // r2 with a type error: the check fails and says why.
    await code.fill((await code.inputValue()).replace('pnex.loop();', 'pnex.loop();\n  undeclared_symbol = 1;'));
    await main.getByRole('button', { name: app.t('firmware-save'), exact: true }).click();
    await main.getByRole('button', { name: app.t('firmware-verify'), exact: true }).click();
    await expect(main.getByText(app.tr('firmware-check-failed'))).toBeVisible({ timeout: 10 * 60_000 });
    await expect(main).toContainText('undeclared_symbol');
    await capture('firmware-verify-error', { caption: 'Compile error reported in the IDE' });

    await app.goto('/firmware');
    const row = page.locator('main tr').filter({ hasText: name });
    await row.getByRole('button', { name: app.t('firmware-delete') }).click();
    await confirmDialog(page, app.t('firmware-confirm-delete-title')).getByRole('button', { name: app.t('firmware-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
