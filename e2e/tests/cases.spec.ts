// Case workspace (cases-workspace.md): events placed on the timeline,
// objects attached by drag and drop with a role, undated pieces, the
// inspector, a case opened from an alert, the frame (pages, thresholds,
// frozen states, purge), the Markdown export and the re-import. Objects
// are titled with the test prefix: the ontology is not swept between runs.
import { expect, test } from '../src/fixtures.ts';
import { dialog } from '../src/pages/shell.ts';
import { DocEditor } from '../src/pages/docs.ts';

const instant = (t: string) => ({ from: t, to: t, precision: 'instant' });

test.describe('cases', { tag: ['@cases', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    // Parallel workers may install at once on a fresh base: retry once.
    for (const pack of ['maintenance', 'investigation']) {
      await api.post(`/ontology/packs/${pack}/install`, {}).catch(() =>
        api.post(`/ontology/packs/${pack}/install`, {}),
      );
    }
  });

  test('places an event, attaches an object by drag and drop, changes its role, detaches it', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const run = Date.now().toString(36);
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} ${run} pump P-12`,
      properties: { serial: 'S-12', status: 'running' },
    });

    // New case from the list: the header button opens the title dialog.
    await app.goto('/cases');
    await page.getByRole('button', { name: app.t('cases-new'), exact: true }).click();
    await page.getByTestId('case-new-title').fill(`${prefix} ${run} low pressure`);
    await page.getByTestId('case-new-title').press('Enter');
    await expect(page).toHaveURL(/\/cases\/[0-9a-f-]{36}/);
    const caseId = page.url().split('/').pop()!;
    const inspector = page.getByTestId('case-inspector');
    await expect(page.getByTestId('case-empty')).toContainText(app.t('cw-empty-title'));
    await capture('case-empty', { caption: 'An empty case guides the first gesture' });

    // A click on the Events line places an instant, selected and titled at once.
    const lane = page.getByTestId('case-events-lane');
    const box = (await lane.boundingBox())!;
    await page.mouse.click(box.x + box.width * 0.5, box.y + 12);
    const title = inspector.getByTestId('case-event-title');
    await expect(title).toHaveValue(app.t('cw-new-event'));
    await title.fill('Pressure drop');
    await title.press('Enter');
    await title.blur();
    await expect(page.getByTestId('case-event')).toContainText('Pressure drop');

    // Drag the pump from the "+" panel onto the event.
    await page.getByTestId('case-add').click();
    await page.getByTestId('case-search').fill(`${prefix} ${run} pump`);
    const chip = page.getByTestId('case-object').filter({ hasText: `${prefix} ${run} pump P-12` });
    await expect(chip).toBeVisible();
    const from = (await chip.boundingBox())!;
    const to = (await page.getByTestId('case-event').first().boundingBox())!;
    await page.mouse.move(from.x + 20, from.y + 10);
    await page.mouse.down();
    await page.mouse.move(from.x + 60, from.y + 30, { steps: 4 });
    await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
    await page.mouse.up();
    await page.getByTestId('case-event').first().click();
    const attachment = inspector.getByTestId('case-attachment').filter({ hasText: `${prefix} ${run} pump P-12` });
    await expect(attachment).toBeVisible();
    await expect(attachment.getByTestId('case-role')).toHaveValue('evidence');
    await capture('case-event-attached', { caption: 'An event with an object attached as evidence' });

    // Another role; then detached, it stays in the case, undated.
    await attachment.getByTestId('case-role').selectOption('context');
    await expect
      .poll(async () => (await api.get(`/cases/${caseId}/workspace`)).events[0].attachments[0].role)
      .toBe('context');
    await attachment.getByRole('button').last().click();
    await page.getByTestId('case-undated-tab').click();
    await expect(page.getByTestId('case-tray')).toContainText(`${prefix} ${run} pump P-12`);
    const w = await api.get(`/cases/${caseId}/workspace`);
    expect(w.undated.map((o: any) => o.id)).toContain(pump.id);
  });

  test('adds an object to a case from its object page', async ({ app, page, api, prefix }) => {
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump P-7`,
      properties: { serial: 'S-7', status: 'running' },
    });
    const c = await api.post('/ontology/objects', {
      type_key: 'case',
      title: `${prefix} vibration`,
      properties: { status: 'open' },
    });
    await app.goto(`/ontology/object/${pump.id}`);
    await page.getByTestId('add-to-case').click();
    const form = dialog(page, app.t('case-add'));
    await form.getByTestId('add-to-case-case').selectOption(c.id);
    await form.getByRole('button', { name: app.t('case-add-submit'), exact: true }).click();
    await app.expectToast(app.t('case-added'));
    const w = await api.get(`/cases/${c.id}/workspace`);
    expect(w.undated.map((o: any) => o.id)).toContain(pump.id);
  });

  test('a case opened from an alert starts with the alert as its first event', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const c = await api.post('/cases', {
      title: `${prefix} alarm`,
      from: { kind: 'event', at: '2026-10-03T10:02:00Z' },
    });
    const w = await api.get(`/cases/${c.id}/workspace`);
    expect(w.events[0].event.properties.occurred).toEqual(instant('2026-10-03T10:02:00Z'));
    await app.goto(`/cases/${c.id}`);
    await expect(page.getByTestId('case-event')).toContainText(`${prefix} alarm`);
    await page.getByTestId('case-event').first().click();
    await expect(page.getByTestId('case-inspector').getByTestId('case-event-title')).toHaveValue(`${prefix} alarm`);
  });

  test('starts a page from a template in the frame, designates it, freezes it at the conclusion', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} trip ${run}`, properties: { status: 'open' },
    });
    await app.goto(`/cases/${c.id}`);
    await page.getByTestId('case-frame').click();
    const panel = page.getByTestId('case-pages');
    await expect(panel.getByTestId('case-page-template').locator('option[value="five_whys"]')).toHaveCount(1);
    await panel.getByTestId('case-page-template').selectOption('five_whys');
    await panel.getByTestId('case-page-title').fill(`${prefix} why ${run}`);
    await panel.getByTestId('case-page-title').press('Enter');

    const editor = new DocEditor(app, page);
    await editor.waitSynced();
    await expect(editor.content.locator('h2').first()).toHaveText('Problem');
    const docId = editor.id;

    await app.goto(`/cases/${c.id}`);
    await page.getByTestId('case-add').click();
    await page.getByTestId('case-undated-tab').click();
    await expect(page.getByTestId('case-tray')).toContainText(`${prefix} why ${run}`);
    await page.getByTestId('case-add').click();
    await page.getByTestId('case-frame').click();
    await panel.getByTestId('case-page-row').filter({ hasText: `${prefix} why ${run}` })
      .getByTestId('case-page-designate').click();
    await expect(panel.getByTestId('case-page-conclusion')).toBeVisible();
    await capture('case-frame', { caption: 'The case frame: question, processing basis, pages' });

    await page.getByTestId('case-header').locator('[data-status="concluded"]').click();
    await expect.poll(async () => JSON.stringify(await api.get(`/docs/${docId}/versions`)))
      .toContain('"Conclusion"');
  });

  test('an archived case proposes to purge the persons only it holds', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const run = Date.now().toString(36);
    const obj = (type_key: string, title: string, properties = {}) =>
      api.post('/ontology/objects', { type_key, title: `${prefix} ${title} ${run}`, properties });
    const c = await obj('case', 'fraud', { status: 'open', legal_basis: 'legitimate interest' });
    const ann = await obj('person', 'Ann');
    const bob = await obj('person', 'Bob');
    const carl = await obj('person', 'Carl');
    for (const o of [ann, bob]) {
      await api.post(`/cases/${c.id}/attach`, { object_id: o.id });
    }
    // Bob is also known outside the case: he is not proposed.
    await api.post('/ontology/links', { link_type: 'knows', source_id: bob.id, target_id: carl.id });

    await app.goto(`/cases/${c.id}`);
    await page.getByTestId('case-header').locator('[data-status="archived"]').click();
    await page.getByTestId('case-frame').click();
    const panel = page.getByTestId('case-purge');
    await expect(panel).toContainText(`${prefix} Ann ${run}`);
    await expect(panel).not.toContainText(`${prefix} Bob ${run}`);

    await panel.getByTestId('case-purge-start').click();
    await page.getByRole('alertdialog').getByRole('button').last().click();
    await expect.poll(() => api.get(`/ontology/objects/${ann.id}`).then(() => 'kept', () => 'purged'))
      .toBe('purged');
    expect(await api.get(`/ontology/objects/${bob.id}`)).toHaveProperty('id', bob.id);
  });

  test('freezes a state, saves it as Markdown, imports the export as a new case', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const run = Date.now().toString(36);
    const title = `${prefix} audit ${run}`;
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title, properties: { status: 'open', question: 'Who opened the gate?' },
    });
    const gate = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} gate ${run}`, properties: {} });
    await api.post(`/cases/${c.id}/events`, {
      title: 'Gate opened', occurred: instant('2025-05-02T09:00:00Z'), attach: [{ object_id: gate.id }],
    });
    const text = async (d: Promise<any>) => {
      let out = '';
      for await (const chunk of await (await d).createReadStream()) out += chunk.toString();
      return out;
    };

    await app.goto(`/cases/${c.id}`);
    await page.getByTestId('case-frame').click();
    // A case threshold, saved as a number.
    const gap = page.getByTestId('case-thresholds').locator('input').last();
    await gap.fill('120');
    await gap.blur();
    await expect.poll(async () => (await api.get(`/ontology/objects/${c.id}`)).properties.signal_gap_s).toBe(120);

    const frozen = page.getByTestId('case-snapshots');
    await frozen.locator('input').fill('Before the report');
    await page.getByTestId('case-freeze').click();
    const row = frozen.getByTestId('case-snapshot');
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('Before the report');
    await capture('case-frozen-states', { caption: 'Frozen states in the case frame, with their fingerprint' });
    const md = await text(Promise.all([page.waitForEvent('download'), row.getByRole('button', { name: 'Markdown' }).click()]).then(([d]) => d));
    expect(md).toContain(`# ${title}`);
    expect(md).toContain('Gate opened');
    expect(md).toContain('SHA-256');

    // The export, imported back from the list, opens as a new case.
    const json = await text(Promise.all([page.waitForEvent('download'), page.getByTestId('case-export').click()]).then(([d]) => d));
    await app.goto('/cases');
    await page.getByTestId('case-import-file').setInputFiles({ name: 'case.json', mimeType: 'application/json', buffer: Buffer.from(json) });
    await expect(page).toHaveURL(/\/cases\/[0-9a-f-]{36}/);
    const copy = page.url().split('/').pop()!;
    expect(copy).not.toBe(c.id);
    const w = await api.get(`/cases/${copy}/workspace`);
    expect(w.events.map((e: any) => e.event.title)).toEqual(['Gate opened']);
    expect(w.events[0].attachments[0].object.id).toBe(gate.id);
  });

  test('a node moved in the graph keeps its place after a reload, until re-layout', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} layout ${run}`, properties: { status: 'open' },
    });
    const yard = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} yard ${run}`, properties: {} });
    const gate = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} gate ${run}`, properties: {} });
    await api.post(`/cases/${c.id}/events`, {
      title: 'Seen', occurred: instant('2025-05-02T09:00:00Z'),
      attach: [{ object_id: yard.id }, { object_id: gate.id }],
    });
    await app.goto(`/cases/${c.id}`);
    const nodes = page.getByTestId('case-graph-node');
    const centre = async (text: string) => {
      const b = (await nodes.filter({ hasText: text }).locator('circle, rect').first().boundingBox())!;
      return [b.x + b.width / 2, b.y + b.height / 2];
    };
    // Shape of the drawing, whatever its scale: |event–yard| / |event–gate|.
    const shape = async () => {
      const [e, y, g] = [await centre('Seen'), await centre(`yard ${run}`), await centre(`gate ${run}`)];
      return Math.hypot(e[0] - y[0], e[1] - y[1]) / Math.hypot(e[0] - g[0], e[1] - g[1]);
    };
    await expect(nodes.filter({ hasText: `yard ${run}` })).toBeVisible();
    const before = await shape();
    const [x, y] = await centre(`yard ${run}`);
    const panel = (await page.getByTestId('case-graph').boundingBox())!;
    // Toward the middle of the panel, by a good third of the way.
    const [tx, ty] = [x + (panel.x + panel.width / 2 - x) * 0.6, y + (panel.y + panel.height / 2 - y) * 0.6];
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(tx, ty, { steps: 8 });
    await page.mouse.up();
    // The node stays where it was let go.
    const [dx, dy] = await centre(`yard ${run}`);
    expect(Math.hypot(dx - tx, dy - ty)).toBeLessThan(4);
    await expect.poll(async () => Object.keys((await api.get(`/cases/${c.id}/layout`)).positions)).toContain(yard.id);
    const dropped = await shape();
    expect(Math.abs(dropped - before)).toBeGreaterThan(0.05);

    // After a reload, the same arrangement.
    await page.reload();
    await expect(nodes.filter({ hasText: `yard ${run}` })).toBeVisible();
    await expect.poll(async () => Math.abs((await shape()) - dropped)).toBeLessThan(0.02);

    // Re-layout: back to the automatic layout, nothing saved.
    await page.getByRole('button', { name: app.t('cw-graph-relayout'), exact: true }).click();
    await expect.poll(async () => Object.keys((await api.get(`/cases/${c.id}/layout`)).positions)).toEqual([]);
    await expect.poll(async () => Math.abs((await shape()) - before)).toBeLessThan(0.02);
  });
});
