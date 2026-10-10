// Ontology (ontology.md D176–D191): the maintenance pack, typed objects
// created from the explorer, temporal links (open, close, history), the
// local graph, the type editor (a new version), the packs tab and the
// global search. Objects are titled with the test prefix: the ontology is
// not swept between runs, a type cannot be deleted while it has objects.
import { expect, test } from '../src/fixtures.ts';
import { dialog } from '../src/pages/shell.ts';
import { escapeRe } from '../src/pages/notifications.ts';

test.describe('ontology', { tag: ['@ontology', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    // Idempotent at the same version; two workers installing at once may
    // collide on the first install (409): the second attempt finds it done.
    await api.post('/ontology/packs/maintenance/install', {}).catch(() =>
      api.post('/ontology/packs/maintenance/install', {}),
    );
  });

  test('creates a pump, links it, closes the link and reads its history', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const line = await api.post('/ontology/objects', {
      type_key: 'line',
      title: `${prefix} line`,
      properties: { code: 'L2' },
    });

    // Explorer: pick the Pump type, create one through the typed form.
    await app.goto('/ontology');
    await page.getByTestId('onto-type-filter').selectOption('pump');
    await page.getByRole('button', { name: app.t('onto-new-object') }).click();
    const form = dialog(page, app.tr('onto-new-typed'));
    await expect(form).toBeVisible();
    await form.getByTestId('onto-object-title').fill(`${prefix} pump`);
    // Required property missing: refused locally with the field token.
    await form.getByRole('button', { name: app.t('common-save') }).click();
    await expect(form.getByText(app.t('err-required'))).toBeVisible();
    await form.locator('[data-prop="serial"] input').fill('S-12');
    await form.locator('[data-prop="status"] select').selectOption('running');
    await capture('ontology-new-pump', { caption: 'A pump created from the typed form' });
    await form.getByRole('button', { name: app.t('common-save') }).click();

    // Object page.
    await expect(page).toHaveURL(/\/ontology\/object\//);
    await expect(page.getByRole('heading', { name: `${prefix} pump` })).toBeVisible();
    await expect(page.locator('[data-prop="serial"]')).toContainText('S-12');

    // Links: open "feeds" towards the line.
    await page.getByRole('tab', { name: app.t('onto-tab-links') }).click();
    await page.getByTestId('onto-add-link').click();
    const linkForm = dialog(page, app.t('onto-add-link'));
    await linkForm.getByTestId('onto-link-type').selectOption('feeds:out');
    await linkForm.getByTestId('onto-link-other').selectOption(line.id);
    await linkForm.getByRole('button', { name: app.t('onto-open-link') }).click();
    const row = page.locator('[data-link="feeds"]');
    await expect(row).toContainText(`${prefix} line`);
    await capture('ontology-links', { caption: 'A typed link, valid since now' });

    // Graph: the pump and the line.
    await page.getByRole('tab', { name: app.t('onto-tab-graph') }).click();
    const graph = page.getByTestId('ontology-graph');
    await expect(graph).toBeVisible();
    await expect(graph).toContainText(`${prefix} line`);
    await capture('ontology-graph', { caption: 'Local graph of the object' });

    // Close the link: gone from "now", kept in the history.
    await page.getByRole('tab', { name: app.t('onto-tab-links') }).click();
    await row.getByTestId('onto-close-link').click();
    await app.expectToast(app.t('onto-link-closed'));
    await expect(page.locator('[data-link="feeds"]')).toHaveCount(0);
    await page.getByLabel(app.t('onto-history')).check();
    await expect(page.locator('[data-link="feeds"]')).toHaveCount(1);
    await expect(page.locator('[data-link="feeds"]')).toContainText('→');

    // Both ends see the closed link in their history (API side).
    const hist = await api.get(`/ontology/objects/${line.id}/links?history=true`);
    expect(hist.some((l: any) => l.link_type === 'feeds' && l.valid_to)).toBe(true);
  });

  test('edits a type and lists the packs', async ({ app, page, api, prefix }) => {
    await app.goto('/ontology?tab=schema');
    await page.getByRole('row', { name: /machine/ }).first().click();
    await expect(page).toHaveURL(/\/ontology\/types\/machine/);
    const before = await api.get('/ontology/types/machine');

    // A new property appended, saved as a new version.
    await page.getByTestId('onto-add-property').click();
    const rows = page.locator('[data-property-row]');
    const last = rows.last();
    // Unique per run: the type is not swept, a reused key is a duplicate.
    const key = `p_${Date.now().toString(36)}${prefix.endsWith('fr') ? 'f' : 'e'}`;
    await last.getByTestId('onto-prop-key').fill(key);
    await last.getByTestId('onto-prop-kind').selectOption('number');
    await page.getByTestId('onto-save-type').click();
    await app.expectToast(app.t('onto-type-saved'));
    const after = await api.get('/ontology/types/machine');
    expect(after.version).toBe(before.version + 1);
    expect(after.def.properties.some((p: any) => p.key === key)).toBe(true);

    // Packs: the maintenance pack is installed.
    await app.goto('/ontology?tab=packs');
    const card = page.locator('[data-pack="maintenance"]');
    await expect(card).toBeVisible();
    await expect(card.getByText(app.tr('onto-pack-installed'))).toBeVisible();
  });

  test('global search finds an object', async ({ app, page, api, prefix }) => {
    // Unique per run: objects are not swept between runs.
    const title = `${prefix} plant ${Date.now().toString(36)}`;
    const site = await api.post('/ontology/objects', {
      type_key: 'site',
      title,
      properties: {},
    });
    await app.goto('/');
    const search = page.getByPlaceholder(app.t('search-placeholder')).first();
    await search.fill(title);
    await expect(page.getByText(app.t('search-group-object'), { exact: true }).first()).toBeVisible();
    await page.getByRole('button', { name: new RegExp(`^${escapeRe(title)}`) }).click();
    await expect(page).toHaveURL(new RegExp(`/ontology/object/${site.id}`));
  });
});

test.describe('type dashboards', { tag: ['@ontology', '@dashboards'] }, () => {
  test('a pump dashboard is drawn for the pump picked by the viewer', async ({ app, page, api, prefix }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() =>
      api.post('/ontology/packs/maintenance/install', {}),
    );
    const title = `${prefix} pump ${Date.now().toString(36)}`;
    const pump = await api.post('/ontology/objects', { type_key: 'pump', title, properties: { serial: 'S' } });
    const dash = await api.post('/dashboards', { name: `${prefix} pump dashboard` });
    await api.patch(`/dashboards/${dash.id}`, {
      expected_version_number: dash.current_version_number,
      layout: {
        canvas: { width: 800, height: 600 },
        object_type: 'pump',
        widgets: [
          {
            id: 'w1',
            type: 'gauge',
            title: 'Temperature',
            x: 20,
            y: 20,
            w: 200,
            h: 200,
            source: [{ metric: '', device_id: '', window: '1h', object_property: 'temperature' }],
          },
        ],
      },
    });
    await app.goto(`/dashboards?id=${dash.id}`);
    const picker = page.getByTestId('dashboard-object-picker');
    await expect(picker).toBeVisible();
    await picker.selectOption(pump.id);
    await expect(picker).toHaveValue(pump.id);
    await expect(page.getByText('Temperature').first()).toBeVisible();
    await api.delete(`/dashboards/${dash.id}`).catch(() => {});
  });
});
