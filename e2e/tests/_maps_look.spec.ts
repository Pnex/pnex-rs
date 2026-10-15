// Dev helper: the main map and a case map side by side, same list panel
// (runs only with PNEX_E2E_EXPLORE=1; read-only).
import { test } from '../src/fixtures.ts';

const DIR = process.env.TOUR_DIR ?? 'test-results/maps';

test('maps look', async ({ app, page, api }) => {
  test.setTimeout(120_000);
  await app.goto('/map');
  await page.waitForTimeout(2500);
  await page.screenshot({ path: `${DIR}/main-map.png` });
  const cases = await api.post<{ rows: { object: { id: string; title: string } }[] }>('/ontology/query', {
    type_key: 'case',
    limit: 20,
  });
  for (const [name, re] of [['kadhafi', /Kadhafi/], ['bardella', /Bardella/]] as const) {
    const pick = cases.rows.find((r) => re.test(r.object.title));
    if (!pick) continue;
    await app.goto(`/cases/${pick.object.id}`);
    await page.waitForTimeout(2000);
    await page.getByRole('button', { name: /^(Map|Carte)$/ }).first().click();
    await page.waitForTimeout(3500);
    await page.screenshot({ path: `${DIR}/case-${name}.png` });
    const search = page.getByPlaceholder(/located|localisé/i);
    if (await search.isVisible()) {
      await search.fill('Sarkozy');
      await page.waitForTimeout(600);
      await page.screenshot({ path: `${DIR}/case-${name}-search.png` });
    }
  }
});
