// Application shell: task-grouped navigation (design-system.md §5).
import { expect, test } from '../src/fixtures.ts';

test.describe('shell', { tag: '@shell' }, () => {
  test('navigation groups: current group open, others fold out on click', async ({ app, page }) => {
    await app.goto('/flows');
    // The visible menu (the mobile drawer holds a hidden copy).
    const nav = page
      .locator('nav:visible')
      .filter({ has: page.getByRole('link', { name: app.t('nav-dashboard'), exact: true }) })
      .first();
    const automation = nav.getByRole('button', { name: app.t('nav-automation'), exact: true });
    const monitor = nav.getByRole('button', { name: app.t('nav-group-monitor'), exact: true });
    await expect(automation).toHaveAttribute('aria-expanded', 'true');
    await expect(nav.getByRole('link', { name: app.t('nav-flows'), exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(monitor).toHaveAttribute('aria-expanded', 'false');
    await expect(nav.getByRole('link', { name: app.t('nav-dashboards'), exact: true })).toHaveCount(0);

    await monitor.click();
    await expect(monitor).toHaveAttribute('aria-expanded', 'true');
    await nav.getByRole('link', { name: app.t('nav-dashboards'), exact: true }).click();
    await expect(page).toHaveURL(/\/dashboards/);
    await expect(page.locator('main h1')).toHaveText(app.t('nav-dashboards'));
  });
});
