// First launch: nothing stored. The app speaks the phone's language, asks
// for the server, pins its CA after the fingerprint screen (TOFU) and lands
// on the login screen.
import * as android from '../src/android.ts';
import { API_BASE, expect, test } from '../src/fixtures-android.ts';
import { t } from '../src/i18n.ts';

test.describe('android onboarding', { tag: ['@android', '@onboarding'] }, () => {
  test.use({ session: 'fresh' });

  test('server picker → CA trust → login, in the OS language', async ({ page, serverCa }) => {
    const locale = android.deviceLocale().startsWith('fr') ? 'fr-FR' : 'en-US';

    await expect(page.getByText(t(locale, 'server-url-title'), { exact: true })).toBeVisible({ timeout: 30_000 });
    await page.getByPlaceholder(t(locale, 'server-url-placeholder')).first().fill(API_BASE);
    await page.getByRole('button', { name: t(locale, 'server-url-connect') }).click();

    // The trust screen only follows a failed HTTPS probe: a plain-http
    // stack goes straight to the login.
    const pinsCa = !!serverCa && API_BASE.startsWith('https:');
    if (pinsCa) {
      await expect(page.getByText(t(locale, 'trust-ca-title'), { exact: true })).toBeVisible();
      await page.getByRole('button', { name: t(locale, 'trust-ca-accept') }).click();
    }
    await expect(page.getByRole('button', { name: t(locale, 'login-signin') })).toBeVisible();

    const stored = android.readStorage();
    expect(stored['pnex.api_base']).toBe(API_BASE);
    if (pinsCa) expect(stored['pnex.server_ca']?.trim()).toBe(serverCa.trim());
  });
});
