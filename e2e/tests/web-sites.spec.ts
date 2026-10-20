// Websites and their navigation studio (site-recipes.md D323–D326): a site
// added from the list, its starter rule saved, a step added by the action
// strip, the code tab, two versions, then the rule deleted. No run is played:
// that would depend on an outside site.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

test.describe('websites studio', { tag: '@sites' }, () => {
  test('rule saved, edited, versioned and deleted', async ({ app, page, prefix }) => {
    // Unique per attempt: a failed run leaves its rule behind.
    const host = `${prefix.toLowerCase()}-${Date.now().toString(36)}.example.org`;
    await app.goto('/web-sites');
    await page.getByRole('main').getByRole('button', { name: app.t('web-sites-add') }).click();
    const dlg = dialog(page, app.t('web-sites-add'));
    await dlg.getByRole('textbox').fill(host);
    await dlg.getByRole('button', { name: app.t('web-sites-open-studio') }).click();
    await expect(page).toHaveURL(new RegExp(`/web-sites/${host.replace(/\./g, '\\.')}$`));

    // Starter rule, saved as version 1.
    const save = page.getByRole('button', { name: app.t('web-sites-save'), exact: true });
    await save.click();
    await expect(page.getByText('v1', { exact: true })).toBeVisible();
    await expect(save).toBeDisabled();

    // A step dragged in from the strip (here: clicked), saved as version 2.
    const blocks = page.locator('ol > li[draggable]');
    const before = await blocks.count();
    await page.getByRole('button', { name: `+ ${app.t('web-sites-action-wait')}` }).click();
    await expect(blocks).toHaveCount(before + 1);
    await save.click();
    await expect(page.getByText('v2', { exact: true })).toBeVisible();

    // The code tab writes the rule as YAML.
    await page.getByRole('button', { name: app.t('web-sites-code'), exact: true }).click();
    await expect(page.locator('textarea')).toHaveValue(new RegExp(`site: "${host.replace(/\./g, '\\.')}"`));
    await expect(page.locator('textarea')).toHaveValue(/- wait: \{ms: 1000\}/);
    // The column's tab (the bar's « Rule » button hides the column).
    await page.locator('aside').getByRole('button', { name: app.t('web-sites-rule'), exact: true }).click();

    // Both versions are listed.
    await page.getByRole('button', { name: app.t('web-sites-versions'), exact: true }).click();
    const drawer = page.locator('aside').filter({ hasText: app.t('web-sites-version-open') });
    await expect(drawer.locator('li').getByText('v1', { exact: true })).toBeVisible();
    await expect(drawer.locator('li').getByText('v2', { exact: true })).toBeVisible();
    await drawer.getByRole('button', { name: '✕' }).click();

    // Deleted from the rule settings; the site leaves the list.
    await page.getByText(app.t('web-sites-settings')).click();
    await page.getByRole('button', { name: app.t('web-sites-delete-recipe') }).click();
    await confirmDialog(page, app.t('web-sites-delete-recipe'))
      .getByRole('button', { name: app.t('web-sites-delete-recipe') })
      .click();
    await app.goto('/web-sites');
    await expect(page.locator('main tr').filter({ hasText: host })).toHaveCount(0);
  });
});
