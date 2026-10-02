import { defineConfig, devices } from '@playwright/test';
import { BASE_URL } from './src/env.ts';

const CI = !!process.env.CI;

export default defineConfig({
  testDir: './tests',
  // `_explore.spec.ts` is a dev helper (bun run explore), never part of a run.
  testIgnore: process.env.PNEX_E2E_EXPLORE ? [] : ['**/_*.spec.ts'],
  globalSetup: './global-setup.ts',
  outputDir: './test-results',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  // The stack under test is a single dev box: keep the load modest.
  workers: Number(process.env.PNEX_E2E_WORKERS ?? 2),
  reporter: CI
    ? [['list'], ['html', { open: 'never' }], ['junit', { outputFile: 'test-results/junit.xml' }]]
    : [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: BASE_URL,
    // A missing element fails fast instead of eating a long test timeout.
    actionTimeout: 15_000,
    ignoreHTTPSErrors: true,
    viewport: { width: 1600, height: 1000 },
    // Crisp captures; PNEX_E2E_DPR=1 for faster plain test runs.
    deviceScaleFactor: Number(process.env.PNEX_E2E_DPR ?? 2),
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
    launchOptions: {
      // Software WebGL (maplibre, viewers) on headless boxes without a GPU.
      args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader'],
    },
  },
  projects: [
    {
      name: 'en',
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 }, locale: 'en-US' },
    },
    {
      // French UI: only the tests tagged @i18n (locale coverage, not features).
      name: 'fr',
      grep: /@i18n/,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 }, locale: 'fr-FR' },
    },
  ],
});
