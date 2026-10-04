// `pnex://` deep links (return from the system browser after the Rauthy
// login) reach the running app: same process, same screen, no second
// instance (singleTask, patch-android-manifest.py).
import * as android from '../src/android.ts';
import { expect, test } from '../src/fixtures-android.ts';

test.describe('android deep link', { tag: ['@android', '@deeplink'] }, () => {
  test('pnex://return resumes the running app', async ({ app, page }) => {
    await app.goto('/devices');
    await expect(page.locator('main h1').first()).toHaveText(app.tr('nav-devices'));
    const pid = android.appPid();

    android.shell(`am start -W -a android.intent.action.VIEW -d pnex://return ${android.PACKAGE}`);

    await expect.poll(() => android.focusedWindow(), { timeout: 10_000 }).toContain(android.PACKAGE);
    expect(android.appPid()).toBe(pid);
    await expect(page.locator('main h1').first()).toHaveText(app.tr('nav-devices'));
  });
});
