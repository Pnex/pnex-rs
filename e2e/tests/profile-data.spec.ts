// Profile preferences (language switch) and fluid mixtures (CoolProp).
import { expect, test } from '../src/fixtures.ts';
import { t } from '../src/i18n.ts';
import { dialog } from '../src/pages/shell.ts';

test.describe('profile', { tag: '@profile' }, () => {
  // The language is a user preference: never run alongside itself.
  test.describe.configure({ mode: 'serial' });

  test('language switch to French and back', async ({ app, page, api }) => {
    const language = () => api.get<{ profile: { language: string } }>('/user-info').then((u) => u.profile.language);
    await app.goto('/profile');
    // Picking a language previews it at once; saving persists it.
    await page.getByRole('combobox', { name: app.t('profile-language') }).selectOption('fr');
    await expect(page.locator('main h1')).toHaveText(t('fr-FR', 'nav-profile'));
    await page.getByRole('main').getByRole('button', { name: t('fr-FR', 'common-save') }).click();
    await expect.poll(language).toBe('fr');

    await page.getByRole('combobox', { name: t('fr-FR', 'profile-language') }).selectOption('en');
    await expect(page.locator('main h1')).toHaveText(t('en-US', 'nav-profile'));
    await page.getByRole('main').getByRole('button', { name: t('en-US', 'common-save') }).click();
    await expect.poll(language).toBe('en');
  });
});

test.describe('mixtures', { tag: '@mixtures' }, () => {
  test('propane/ethane mixture: created, listed, deleted', async ({ app, page, prefix, capture }) => {
    const name = `${prefix} loop`;
    await app.goto('/mixtures');
    await page.getByRole('main').getByRole('button', { name: app.t('mixtures-new') }).click();
    const dlg = dialog(page, app.t('mixtures-new-title'));
    await dlg.getByRole('textbox', { name: app.t('mixtures-name-placeholder') }).fill(name);
    const save = dlg.getByRole('button', { name: app.t('mixtures-save'), exact: true });
    await expect(save).toBeEnabled();
    await capture('mixture-form', { caption: 'Propane / ethane mixture' });
    await save.click();
    await expect(dlg).toBeHidden();

    const row = page.locator('main tr').filter({ hasText: name });
    await expect(row).toBeVisible();
    await row.getByRole('button', { name: app.t('mixtures-delete') }).click();
    await row.getByRole('button', { name: app.t('mixtures-confirm-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
