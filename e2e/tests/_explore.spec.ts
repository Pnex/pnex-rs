// Dev helper while writing a spec: opens ROUTE, runs the optional ACTIONS
// (async JS body with `page` in scope), then prints the aria snapshot of
// <main> and saves a screenshot to test-results/explore.png.
//   ROUTE=/functions ACTIONS="await page.getByRole('button',{name:'New function'}).click()" bun run explore
import { test } from '../src/fixtures.ts';

test('explore', async ({ app, page }) => {
  await app.goto(process.env.ROUTE ?? '/');
  await page.waitForTimeout(1500);
  if (process.env.ACTIONS) {
    const fn = new Function('page', `return (async () => { ${process.env.ACTIONS} })()`);
    await fn(page);
    await page.waitForTimeout(1200);
  }
  const scope = process.env.SCOPE ?? 'body';
  console.log((await page.locator(scope).first().ariaSnapshot()).slice(0, Number(process.env.MAXC ?? 6000)));
  await page.screenshot({ path: 'test-results/explore.png' });
});
