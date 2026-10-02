// Real sign-in through Rauthy (no injected session), then sign-out.
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog } from '../src/pages/shell.ts';
import { EMAIL, PASSWORD } from '../src/env.ts';

test.describe('auth', { tag: ['@auth', '@i18n'] }, () => {
  test.use({ authenticated: false });

  test('sign in with Rauthy, land on the app, sign out', async ({ app, page, capture }) => {
    await app.goto('/');
    const signIn = page.getByRole('button', { name: app.t('login-signin') });
    await expect(signIn).toBeVisible();
    await capture('login', { caption: 'Sign-in page' });

    await signIn.click();
    // Rauthy login (Svelte page): e-mail first, then the password. Typing
    // before hydration submits the bare HTML form ("Content type error").
    await page.waitForURL(/\/auth\/v1\//);
    await page.waitForLoadState('networkidle');
    const email = page.locator('input[type=email]:visible, input[name=email]:visible').first();
    await email.fill(EMAIL);
    await email.press('Enter');
    const password = page.locator('input[type=password]:visible').first();
    await password.fill(PASSWORD);
    await password.press('Enter');

    // Back on the app through /auth/callback.
    await expect(app.logout).toBeVisible({ timeout: 30_000 });
    await expect(page).not.toHaveURL(/\/auth\//);

    await app.logout.click();
    const confirm = confirmDialog(page, app.t('shell-logout-confirm-title'));
    await confirm.getByRole('button', { name: app.t('shell-logout-confirm-action') }).click();
    await expect(page.getByRole('button', { name: app.t('login-signin') })).toBeVisible({ timeout: 30_000 });
  });
});
