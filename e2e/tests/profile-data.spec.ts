// Profile preferences (language, theme) and fluid mixtures (CoolProp).
import { expect, test } from '../src/fixtures.ts';
import { t } from '../src/i18n.ts';
import { dialog } from '../src/pages/shell.ts';

test.describe('profile', { tag: '@profile' }, () => {
  // Language and theme are user preferences: never run alongside themselves.
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

  test('dark theme applies, survives a reload, back to light', async ({ app, page, api }) => {
    const before = (await api.get<{ profile: { theme: string } }>('/user-info')).profile.theme;
    const html = page.locator('html');
    const theme = page.getByRole('combobox', { name: app.t('profile-theme') });
    const save = page.getByRole('main').getByRole('button', { name: app.t('common-save'), exact: true });
    try {
      await app.goto('/profile');
      await theme.selectOption('dark');
      await save.click();
      await expect(html).toHaveAttribute('data-theme', 'dark');
      await expect.poll(async () => (await api.get('/user-info')).profile.theme).toBe('dark');

      await page.reload();
      await app.settle();
      await expect(html).toHaveAttribute('data-theme', 'dark');
      // Dark surfaces, not just the attribute.
      const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
      const [r, g, b] = bg.match(/\d+/g)!.map(Number);
      expect(r + g + b).toBeLessThan(200);

      await theme.selectOption('light');
      await save.click();
      await expect(html).toHaveAttribute('data-theme', 'light');
    } finally {
      await api.patch('/profile', { theme: before });
    }
  });
});

test.describe('mixtures', { tag: '@mixtures' }, () => {
  test('propane/ethane mixture: created, listed, deleted', async ({ app, page, prefix, capture }) => {
    const name = `${prefix} loop`;
    await app.goto('/mixtures');
    await page.getByRole('main').getByRole('button', { name: app.t('mixtures-new') }).click();
    const dlg = dialog(page, app.t('mixtures-new-title'));
    await dlg.getByRole('textbox', { name: app.t('mixtures-name') }).fill(name);
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
