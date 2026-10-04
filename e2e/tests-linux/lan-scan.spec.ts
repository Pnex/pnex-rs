// First launch, found by the LAN scan instead of typing the URL: the scan
// lists the stack, "Connect" pins its CA after the fingerprint screen (TOFU)
// and lands on the login screen. Skipped when the stack is not on a private
// IPv4 LAN address (nothing a scan could find).
import { BASE_URL } from '../src/env.ts';
import { expect, test } from '../src/fixtures-linux.ts';
import { t } from '../src/i18n.ts';
import { systemLocale } from '../src/linux.ts';

const host = new URL(BASE_URL).hostname;
const lan = /^(10\.\d+|192\.168|172\.(1[6-9]|2\d|3[01]))\.\d+\.\d+$/.test(host);

test.describe('linux LAN scan', { tag: ['@linux', '@onboarding'] }, () => {
  test.use({ session: 'fresh' });

  test('scan → connect → CA trust → login', async ({ desktop, serverCa }) => {
    test.skip(!lan, `${host} is not a private IPv4 address`);
    const locale = systemLocale();
    const b = desktop.browser;
    const prefix = host.split('.').slice(0, 3).join('.') + '.';

    await desktop.expectVisible(t(locale, 'server-url-title'), '*', 30_000);
    await desktop.fill(
      `//p[normalize-space()=${JSON.stringify(t(locale, 'scan-prefix-label'))}]/following-sibling::div//input`,
      prefix,
    );
    await desktop.click(t(locale, 'scan-toggle'));

    // The stack answers as `https://<ip>` behind the edge, `http://<ip>:5150`
    // without it.
    const row = b.$(`//p[normalize-space()=${JSON.stringify(BASE_URL)}]/ancestor::div[2]`);
    await row.waitForDisplayed({ timeout: 90_000, timeoutMsg: `${BASE_URL} not found by the scan` });
    await row.$(`.//button[normalize-space()=${JSON.stringify(t(locale, 'scan-connect'))}]`).click();

    const pinsCa = !!serverCa && BASE_URL.startsWith('https:');
    if (pinsCa) {
      await desktop.expectVisible(t(locale, 'trust-ca-title'), '*', 30_000);
      await desktop.click(t(locale, 'trust-ca-accept'));
    }
    await desktop.byText(t(locale, 'login-signin'), 'button').waitForDisplayed({ timeout: 15_000 });

    const stored = desktop.app.readStorage();
    expect(stored['pnex.api_base']).toBe(BASE_URL);
    if (pinsCa) expect(stored['pnex.server_ca']?.trim()).toBe(serverCa.trim());
  });
});
