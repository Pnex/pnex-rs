// Accessibility report (axe-core) of every route. Report-only: violations
// are attached to the test and summed in annotations; set
// PNEX_E2E_A11Y_STRICT=1 to fail on serious/critical ones.
import { AxeBuilder } from '@axe-core/playwright';
import { expect, test } from '../src/fixtures.ts';
import { ROUTES } from '../src/routes.ts';

const STRICT = process.env.PNEX_E2E_A11Y_STRICT === '1';

test.describe('a11y', { tag: '@a11y' }, () => {
  for (const route of ROUTES) {
    test(`axe ${route.path}`, async ({ app, page }, info) => {
      await app.goto(route.path);
      await page.waitForTimeout(800);
      const result = await new AxeBuilder({ page }).include('main').analyze();
      const summary = result.violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        nodes: v.nodes.length,
        help: v.help,
        targets: v.nodes.map((n) => n.target.join(' ')).slice(0, 5),
      }));
      await info.attach('axe-violations.json', { body: JSON.stringify(summary, null, 2), contentType: 'application/json' });
      for (const v of summary) info.annotations.push({ type: `axe:${v.impact}`, description: `${v.id} ×${v.nodes} — ${v.help}` });
      if (STRICT) {
        expect(summary.filter((v) => v.impact === 'serious' || v.impact === 'critical')).toEqual([]);
      }
    });

    // The creation dialog behind the list "Add" button, when it opens one
    // (O21: forms in dialogs were never audited). Pages that create without
    // a dialog (flows, dashboards: swept resources) are skipped.
    test(`axe dialog ${route.path}`, async ({ app, page }, info) => {
      await app.goto(route.path);
      const add = page.getByTestId('list-add');
      test.skip(!(await add.isVisible({ timeout: 3000 }).catch(() => false)), 'no add button');
      await add.click();
      const dialog = page.getByRole('dialog');
      test.skip(!(await dialog.first().isVisible({ timeout: 2000 }).catch(() => false)), 'no dialog');
      await page.waitForTimeout(400);
      const result = await new AxeBuilder({ page }).include('[role="dialog"]').analyze();
      const summary = result.violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        nodes: v.nodes.length,
        help: v.help,
        targets: v.nodes.map((n) => n.target.join(' ')).slice(0, 5),
      }));
      await info.attach('axe-violations.json', { body: JSON.stringify(summary, null, 2), contentType: 'application/json' });
      for (const v of summary) info.annotations.push({ type: `axe:${v.impact}`, description: `${v.id} ×${v.nodes} — ${v.help}` });
      if (STRICT) {
        expect(summary.filter((v) => v.impact === 'serious' || v.impact === 'critical')).toEqual([]);
      }
    });
  }
});
