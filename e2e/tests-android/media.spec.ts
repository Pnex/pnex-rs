// Media capture on the phone: Take 360 (guided overlay over the live camera)
// and the system camera intent of "Take a photo".
import * as android from '../src/android.ts';
import { expect, test } from '../src/fixtures-android.ts';

test.describe('android media', { tag: ['@android', '@media'] }, () => {
  test.beforeEach(() => {
    // The runtime permission dialog is native UI: grant it once by hand.
    test.skip(!android.cameraGranted(), 'CAMERA permission not granted to the app');
  });

  test('Take 360: guided start, live camera, no debug line, cancel', async ({ app, page }) => {
    await app.goto('/media');
    await page.getByRole('button', { name: app.t('media-take360') }).click();

    // Before the start: the step-by-step card with one primary action.
    await expect(page.getByText(app.t('media-take360-intro-title'), { exact: true })).toBeVisible();
    await expect(page.locator('#p360-video')).toHaveAttribute('poster', /^data:image\/gif/);
    await expect.poll(() => page.evaluate(() => (window as any).__p360Ready === true), { timeout: 15_000 }).toBe(true);
    // Shipped UI: the raw diagnostics line only exists with feature `diag`.
    await expect(page.locator('#p360-debug')).toHaveCount(0);

    await page.getByRole('button', { name: app.t('media-take360-start') }).click();
    await expect(page.getByText(app.t('media-take360-hint-auto'), { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: app.t('media-take360-restart-here') })).toBeVisible();

    await page.getByRole('button', { name: app.t('media-take360-cancel') }).click();
    await expect(page.locator('#p360-video')).toHaveCount(0);
    await expect(page.getByRole('button', { name: app.t('media-take360') })).toBeVisible();
  });

  test('Take a photo, then back without shooting: no upload left pending', async ({ app, page }) => {
    await app.goto('/media');
    await page.getByRole('button', { name: app.t('media-camera') }).click();

    // The system camera takes the focus…
    await expect.poll(() => android.focusedWindow(), { timeout: 15_000 }).not.toContain(android.PACKAGE);
    // A user looks at the viewfinder for a moment before giving up.
    await page.waitForTimeout(3_000);
    // …then the user comes back without a photo (relaunch = back to the app
    // task, no input injection needed).
    android.launch();
    await expect.poll(() => android.focusedWindow(), { timeout: 15_000 }).toContain(android.PACKAGE);

    // The pending indicator goes away instead of spinning for 10 minutes.
    await expect(page.getByText(app.t('media-upload-progress'))).toHaveCount(0, { timeout: 10_000 });
  });
});
