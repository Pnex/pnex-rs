// Exploration tour of the labels (D218) on every CRUD list: seeds two
// labelled objects through the API, then captures each list, the label
// filter, the labels dialog and the `#value` global search. Dev helper
// (runs only with PNEX_E2E_EXPLORE=1); screenshots go to TOUR_DIR.
import { expect, test } from '../src/fixtures.ts';

const DIR = process.env.TOUR_DIR ?? 'test-results/labels-tour';
const LISTS = [
  '/functions',
  '/firmware',
  '/controls',
  '/pages',
  '/streams',
  '/models',
  '/notifications',
  '/annotations',
  '/sites',
  '/cases',
  '/ontology',
  '/flows',
  '/devices',
];

test('labels tour', async ({ app, page, api, t }) => {
  test.setTimeout(240_000);
  const tag = `tour${Date.now().toString(36)}`;
  const tpl = await api.post<{ id: string }>('/notify/templates', {
    name: `Tour template ${tag}`,
    subject: 's',
    body: 'b',
    vars: [],
  });
  await api.put(`/resources/notify_template/${tpl.id}/labels`, { labels: { site: tag } });
  const doc = await api.post<{ id: string }>('/docs', { title: `Tour page ${tag}` });
  await api.put(`/resources/doc/${doc.id}/labels`, { labels: { site: tag, critique: null } });

  const filter = t('resources-label-filter-placeholder');
  const missing: string[] = [];
  const noSearch: string[] = [];
  for (const route of LISTS) {
    await app.goto(route);
    await page.waitForTimeout(1500);
    const name = route.replaceAll('/', '_') || '_home';
    await page.screenshot({ path: `${DIR}/list${name}.png`, fullPage: true });
    if (!(await page.getByPlaceholder(filter).first().isVisible().catch(() => false))) {
      missing.push(route);
    }
    // The label filter is a search input too: a list needs at least two.
    const searches = await page.locator('main input[type="search"]:visible').count();
    if (searches < 2) noSearch.push(`${route} (${searches})`);
  }
  console.log(`lists without the label filter: ${JSON.stringify(missing)}`);
  console.log(`lists without a name search: ${JSON.stringify(noSearch)}`);

  // Pages: chips under the title, filter narrows the list.
  await app.goto('/pages');
  await expect(page.getByText(`site | ${tag}`).first().or(page.getByText(tag).first())).toBeVisible();
  await page.getByPlaceholder(filter).first().fill(`site:${tag}`);
  await page.getByPlaceholder(filter).first().press('Enter');
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${DIR}/pages-filtered.png`, fullPage: true });

  // Notification templates tab: chip, then the labels dialog.
  await app.goto('/notifications');
  await page.getByRole('tab', { name: t('notify-tab-templates') }).click();
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${DIR}/notify-templates.png`, fullPage: true });
  const row = page.locator('main tr').filter({ hasText: `Tour template ${tag}` });
  await row.getByRole('button', { name: t('resources-labels-title') }).click();
  await page.waitForTimeout(800);
  await page.screenshot({ path: `${DIR}/labels-dialog.png`, fullPage: true });
  await page.keyboard.press('Escape');

  // Global search: `#value` shows the Labels group.
  await app.goto('/');
  const search = page.getByPlaceholder(t('search-placeholder')).first();
  await search.fill(`#${tag}`);
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${DIR}/search-hash.png`, fullPage: true });
  await expect(page.getByText(t('search-group-label')).first()).toBeVisible();
  await expect(page.getByText(`Tour template ${tag}`).first()).toBeVisible();

  await api.delete(`/notify/templates/${tpl.id}`).catch(() => {});
  await api.delete(`/docs/${doc.id}`).catch(() => {});
});
