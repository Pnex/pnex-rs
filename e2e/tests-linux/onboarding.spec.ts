// First launch on the desktop: nothing stored. The app speaks the session's
// language, asks for the server, pins its CA after the fingerprint screen
// (TOFU) and lands on the login screen.
import { BASE_URL } from '../src/env.ts';
import { expect, test } from '../src/fixtures-linux.ts';
import { t } from '../src/i18n.ts';
import { systemLocale } from '../src/linux.ts';

test.describe('linux onboarding', { tag: ['@linux', '@onboarding'] }, () => {
  test.use({ session: 'fresh' });

  test('server picker → CA trust → login, in the OS language', async ({ desktop, serverCa }) => {
    const locale = systemLocale();
    const b = desktop.browser;

    await desktop.expectVisible(t(locale, 'server-url-title'), '*', 30_000);
    await desktop.fill(`input[placeholder="${t(locale, 'server-url-placeholder')}"]`, BASE_URL);
    await desktop.click(t(locale, 'server-url-connect'));

    // The trust screen only follows a failed HTTPS probe: a plain-http
    // stack goes straight to the login.
    const pinsCa = !!serverCa && BASE_URL.startsWith('https:');
    if (pinsCa) {
      // The CA offer follows a failed HTTPS probe plus a CA fetch and a
      // verification: several seconds.
      await desktop.expectVisible(t(locale, 'trust-ca-title'), '*', 30_000);
      await desktop.click(t(locale, 'trust-ca-accept'));
    }
    await desktop.byText(t(locale, 'login-signin'), 'button').waitForDisplayed({ timeout: 15_000 });

    const stored = desktop.app.readStorage();
    expect(stored['pnex.api_base']).toBe(BASE_URL);
    if (pinsCa) expect(stored['pnex.server_ca']?.trim()).toBe(serverCa.trim());
  });
});
