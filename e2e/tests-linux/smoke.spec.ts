// The desktop app boots signed in and every route renders its page (title,
// no Rust panic), in both UI locales.
import { test } from '../src/fixtures-linux.ts';
import { ROUTES } from '../src/routes.ts';

test.describe('linux smoke', { tag: ['@linux', '@smoke', '@i18n'] }, () => {
  test('every route renders', async ({ desktop }) => {
    for (const route of ROUTES) {
      await test.step(route.path, async () => {
        await desktop.goto(route.path);
        if (route.title) await desktop.expectText('main h1', desktop.tr(route.title));
      });
    }
  });
});
