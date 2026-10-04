// Android suite: the native app on a device attached through adb, driven
// through its WebView (Playwright `_android`). One device = one worker.
// Run with `task e2e:android` (e2e/README.md, "Android").
import { defineConfig } from '@playwright/test';

const CI = !!process.env.CI;

export default defineConfig({
  testDir: './tests-android',
  globalSetup: './global-setup.ts',
  outputDir: './test-results-android',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: CI
    ? [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-android' }]]
    : [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-android' }]],
  use: {
    actionTimeout: 15_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'android-en', use: { locale: 'en-US' } },
    // French UI: only the tests tagged @i18n (locale coverage).
    { name: 'android-fr', grep: /@i18n/, use: { locale: 'fr-FR' } },
  ],
});
