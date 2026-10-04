// Native Linux suite: the desktop app (WebKitGTK) driven through
// WebKitWebDriver. One app window at a time (libwebkit2gtk allows a single
// automation context per process; windows would also fight for focus).
// Run with `task e2e:linux` (e2e/README.md, "Linux").
import { defineConfig } from '@playwright/test';

const CI = !!process.env.CI;

export default defineConfig({
  testDir: './tests-linux',
  globalSetup: './global-setup.ts',
  outputDir: './test-results-linux',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-linux' }]],
  projects: [
    { name: 'linux-en', use: { locale: 'en-US' } },
    // French UI: only the tests tagged @i18n (locale coverage).
    { name: 'linux-fr', grep: /@i18n/, use: { locale: 'fr-FR' } },
  ],
});
