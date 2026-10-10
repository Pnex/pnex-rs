// Global search (sidebar): finds resources by name and deep-links to them.
import { expect, test } from '../src/fixtures.ts';
import { escapeRe } from '../src/pages/notifications.ts';

test.describe('search', { tag: ['@search', '@i18n'] }, () => {
  test('finds a function and opens it in the editor', async ({ app, page, api, prefix, capture }) => {
    const name = `${prefix} searchable`;
    await api.post('/functions', {
      name,
      language: 'js',
      code: '// @input value number "Value"\n// @output result number "Result"\nfunction handle(inputs, msg) {\n  return { result: inputs.value };\n}',
    });

    await app.goto('/');
    const search = page.getByPlaceholder(app.t('search-placeholder')).first();
    await search.fill(name);
    // Result buttons read "<name> <kind>".
    const hit = page.getByRole('button', { name: new RegExp(`^${escapeRe(name)}`) });
    await expect(hit).toBeVisible();
    await expect(page.getByText(app.t('search-group-function'), { exact: true }).first()).toBeVisible();
    await capture('global-search', { caption: 'Global search from the sidebar' });

    await hit.click();
    await expect(page).toHaveURL(/\/functions/);
    await expect(page.getByRole('main').locator('input').first()).toHaveValue(name);
  });
});
