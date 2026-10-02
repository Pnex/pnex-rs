// Media library: upload a photo, find it in the list, delete it.
import { expect, test } from '../src/fixtures.ts';
import { makePng } from '../src/fixtures-files.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('media', { tag: '@media' }, () => {
  test('photo: uploaded, listed, deleted', async ({ app, page, browser, prefix, capture }) => {
    const name = `${prefix} site photo`;
    await app.goto('/media');
    await page.getByRole('main').getByRole('button', { name: app.t('media-upload') }).click();
    const dlg = dialog(page, app.t('media-upload-title'));
    await dlg.locator('input[type=file]').setInputFiles({
      name: 'site-photo.png',
      mimeType: 'image/png',
      buffer: await makePng(browser, 'PNeX E2E'),
    });
    await dlg.getByRole('textbox', { name: app.t('media-upload-name-placeholder') }).fill(name);
    await dlg.getByRole('button', { name: app.t('media-upload'), exact: true }).click();
    await expect(dlg).toBeHidden({ timeout: 30_000 });

    const row = page.getByRole('main').getByText(name).first();
    await expect(row).toBeVisible();
    await capture('media-library', { caption: 'Media library with an uploaded photo' });

    await row.click();
    await page.getByRole('button', { name: app.t('media-delete') }).click();
    await confirmDialog(page, app.t('media-delete-confirm-title')).getByRole('button', { name: app.t('common-delete') }).click();
    await expect(page.getByRole('main').getByText(name)).toHaveCount(0);
  });
});
