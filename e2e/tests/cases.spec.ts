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
    const seen = await api.post(`/cases/${c.id}/events`, {
      title: 'Seen', occurred: instant('2025-05-02T09:00:00Z'),
      attach: [{ object_id: yard.id }, { object_id: gate.id }],
    });
    const ev: string = seen.events[0].event.id;
    await app.goto(`/cases/${c.id}`);
    // Sigma draws on a canvas (cases-graph-sigma.md §5): positions and
    // screen points come from window.pnexGraph.
    const host = 'cw-graph-sigma';
    const placed = () =>
      page.evaluate((h) => JSON.parse((window as any).pnexGraph?.positions(h) ?? '{}'), host);
    const centre = async (id: string): Promise<[number, number]> => {
      const p = await page.evaluate(
        ([h, i]) => JSON.parse((window as any).pnexGraph.screenPos(h, i)),
        [host, id],
      );
      return [p.x, p.y];
    };
    // Shape of the drawing, whatever its scale: |event–yard| / |event–gate|.
    const shape = async () => {
      const p = await placed();
      const d = (a: string, b: string) => Math.hypot(p[a][0] - p[b][0], p[a][1] - p[b][1]);
      return d(ev, yard.id) / d(ev, gate.id);
    };
    await expect.poll(async () => Object.keys(await placed()).length).toBe(3);
    const before = await shape();
    const [x, y] = await centre(yard.id);
    const panel = (await page.getByTestId('case-graph').boundingBox())!;
    // Toward the middle of the panel, by a good third of the way.
    const [tx, ty] = [x + (panel.x + panel.width / 2 - x) * 0.6, y + (panel.y + panel.height / 2 - y) * 0.6];
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(tx, ty, { steps: 8 });
    await page.mouse.up();
    // The node stays where it was let go.
    const [dx, dy] = await centre(yard.id);
    expect(Math.hypot(dx - tx, dy - ty)).toBeLessThan(4);
    await expect.poll(async () => Object.keys((await api.get(`/cases/${c.id}/layout`)).positions)).toContain(yard.id);
    const dropped = await shape();
    expect(Math.abs(dropped - before)).toBeGreaterThan(0.05);

    // After a reload, the same arrangement.
    await page.reload();
    await expect.poll(async () => Object.keys(await placed()).length).toBe(3);
    await expect.poll(async () => Math.abs((await shape()) - dropped)).toBeLessThan(0.02);

    // Re-layout: back to the automatic layout, nothing saved.
    await page.getByRole('button', { name: app.t('cw-graph-relayout'), exact: true }).click();
    await expect.poll(async () => Object.keys((await api.get(`/cases/${c.id}/layout`)).positions)).toEqual([]);
    await expect.poll(async () => Math.abs((await shape()) - before)).toBeLessThan(0.02);
  });

  test('double-clicking an object in the graph offers its neighbours, added until undone', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} expand ${run}`, properties: { status: 'open', legal_basis: 'legitimate interest' },
    });
    const ann = await api.post('/ontology/objects', { type_key: 'person', title: `${prefix} Ann ${run}`, properties: {} });
    const acme = await api.post('/ontology/objects', { type_key: 'organization', title: `${prefix} Acme ${run}`, properties: {} });
    await api.post('/ontology/links', { link_type: 'member_of', source_id: ann.id, target_id: acme.id });
    await api.post(`/cases/${c.id}/events`, {
      title: 'Met', occurred: instant('2025-05-02T09:00:00Z'), attach: [{ object_id: ann.id }],
    });
    await app.goto(`/cases/${c.id}`);
    const host = 'cw-graph-sigma';
    const placed = () =>
      page.evaluate((h) => JSON.parse((window as any).pnexGraph?.positions(h) ?? '{}'), host);
    await expect.poll(async () => Object.keys(await placed()).length).toBe(2);
    const p = await page.evaluate(
      ([h, i]) => JSON.parse((window as any).pnexGraph.screenPos(h, i)),
      [host, ann.id],
    );
    await page.mouse.dblclick(p.x, p.y);
    const offer = page.getByTestId('case-expand');
    await expect(offer).toBeVisible();
    await offer.getByRole('checkbox').first().check();
    await page.getByTestId('case-expand-add').click();
    await expect.poll(async () => Object.keys(await placed())).toContain(acme.id);
    await page.getByTestId('case-expand-undo').click();
    await expect.poll(async () => Object.keys(await placed())).not.toContain(acme.id);
  });

  test('removing an object from the case in the inspector takes it out', async ({ app, page, api, prefix }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} remove ${run}`, properties: { status: 'open' },
    });
    const yard = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} yard ${run}`, properties: {} });
    const gate = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} gate ${run}`, properties: {} });
    await api.post(`/cases/${c.id}/events`, {
      title: 'Seen', occurred: instant('2025-05-02T09:00:00Z'), attach: [{ object_id: yard.id }],
    });
    await api.post(`/cases/${c.id}/attach`, { object_id: gate.id });
    await app.goto(`/cases/${c.id}`);
    const host = 'cw-graph-sigma';
    // Drawn, not saved: a node without links sits in the side column, out of positions().
    const drawn = (id: string) =>
      page.evaluate(([h, i]) => JSON.parse((window as any).pnexGraph?.screenPos(h, i) ?? 'null') !== null, [host, id]);
    for (const o of [yard, gate]) await expect.poll(() => drawn(o.id)).toBe(true);
    const members = async () => {
      const w = await api.get(`/cases/${c.id}/workspace`);
      return [
        ...w.undated.map((o: { id: string }) => o.id),
        ...w.events.flatMap((e: { attachments: { object: { id: string } }[] }) => e.attachments.map((a) => a.object.id)),
      ];
    };
    // Both an attached object and an undated one leave on "Remove from the case".
    for (const o of [yard, gate]) {
      const p = await page.evaluate(([h, i]) => JSON.parse((window as any).pnexGraph.screenPos(h, i)), [host, o.id]);
      const remove = page.getByRole('button', { name: app.t('cw-remove-object'), exact: true });
      // Right after a reload of the graph, Sigma may not have redrawn yet.
      await expect(async () => {
        await page.mouse.click(p.x, p.y);
        await expect(remove).toBeVisible({ timeout: 1000 });
      }).toPass();
      await remove.click();
      await expect.poll(members).not.toContain(o.id);
      await expect.poll(() => drawn(o.id)).toBe(false);
    }
  });

  test('without WebGL the graph falls back to the SVG drawing', async ({ app, page, api, prefix }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} nogl ${run}`, properties: { status: 'open' },
    });
    const yard = await api.post('/ontology/objects', { type_key: 'place', title: `${prefix} yard ${run}`, properties: {} });
    await api.post(`/cases/${c.id}/events`, {
      title: 'Seen', occurred: instant('2025-05-02T09:00:00Z'), attach: [{ object_id: yard.id }],
    });
    await page.addInitScript(() => {
      const get = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, ...rest: unknown[]) {
        return /webgl/.test(kind) ? null : get.call(this, kind, ...rest);
      } as typeof get;
    });
    await app.goto(`/cases/${c.id}`);
    await expect(page.getByTestId('case-graph-simplified')).toBeVisible();
    await expect(page.getByTestId('case-graph-node').filter({ hasText: `yard ${run}` })).toBeVisible();
  });

  test('amounts entered on payments draw the money view, with what an intermediary kept', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} money ${run}`, properties: { status: 'open', legal_basis: 'legitimate interest' },
    });
    const [ann, acme, bob] = await Promise.all(
      [['person', 'Ann'], ['organization', 'Acme'], ['person', 'Bob']].map(([type_key, name]) =>
        api.post('/ontology/objects', { type_key, title: `${prefix} ${name} ${run}`, properties: {} }),
      ),
    );
    // Ann pays Acme 1000, Acme passes 400 on to Bob: 1000 paid from the source,
    // 600 stay untraced at Acme.
    const pay = (title: string, at: string, from: string, to: string) =>
      api.post(`/cases/${c.id}/events`, {
        title,
        occurred: instant(at),
        attach: [
          { object_id: from, role: 'payer' },
          { object_id: to, role: 'payee' },
        ],
      });
    await pay('Wire one', '2025-03-01T09:00:00Z', ann.id, acme.id);
    await pay('Wire two', '2025-03-08T09:00:00Z', acme.id, bob.id);

    await app.goto(`/cases/${c.id}`);
    await page.getByTestId('case-view-money').click();
    await expect(page.getByText(app.t('money-empty-title'))).toBeVisible();

    for (const [title, amount] of [['Wire one', '1000'], ['Wire two', '400']]) {
      await page.getByTestId('case-event').filter({ hasText: title }).click();
      const field = page.getByTestId('case-inspector').getByTestId('cw-amount');
      await field.fill(amount);
      await field.press('Enter');
      await field.blur();
    }

    const money = page.getByTestId('case-money');
    await expect(money.getByTestId('money-total')).toContainText(/1[,\u202f]000/); // en: 1,000 · fr: 1 000
    await expect(money.getByTestId('money-untraced')).toContainText('600');
    await capture('money', { caption: 'Payments as a Sankey, untraced money at the intermediary' });
    // An SVG stroke: Playwright's visibility check is unreliable on it.
    await money.getByTestId('money-band').first().click({ force: true });
    await expect(money.getByTestId('money-band-detail')).toBeVisible();
  });
});
