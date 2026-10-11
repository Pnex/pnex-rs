// Cases (cases.md K1, K2, K6): a case opened from one line, members added
// as you type with their role, a dated link between two members, the
// timeline (membership and link events, focus on a member), removal kept
// as history, status changed in place. Objects are titled with the test
// prefix: the ontology is not swept between runs.
import { expect, test } from '../src/fixtures.ts';
import { dialog } from '../src/pages/shell.ts';
import { DocEditor } from '../src/pages/docs.ts';

test.describe('cases', { tag: ['@cases', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() =>
      api.post('/ontology/packs/maintenance/install', {}),
    );
  });

  test('opens a case, gathers members, links them in time and reads the timeline', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    // The ontology is not swept and the prefix is stable across runs: a run
    // tag keeps the quick-add search off the objects of earlier runs.
    const run = Date.now().toString(36);
    await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} ${run} pump P-12`,
      properties: { serial: 'S-12', status: 'running' },
    });
    await api.post('/ontology/objects', {
      type_key: 'line',
      title: `${prefix} ${run} line L2`,
      properties: { code: 'L2' },
    });

    // One line + Enter opens the case and lands in its workspace.
    await app.goto('/cases');
    await capture('cases-list', { caption: 'Cases: open one from a single line' });
    await page.getByTestId('case-new-title').fill(`${prefix} ${run} low pressure`);
    await page.getByTestId('case-new-title').press('Enter');
    await expect(page).toHaveURL(/\/cases\/[0-9a-f-]{36}/);
    const caseId = page.url().split('/').pop()!;

    // Question saved when the field is left.
    await page.getByTestId('case-question').fill('Why did the pressure drop at 10:02?');
    await page.getByTestId('case-question').blur();
    await expect
      .poll(async () => (await api.get(`/ontology/objects/${caseId}`)).properties.question)
      .toBe('Why did the pressure drop at 10:02?');

    // Members as you type: role chip, then Enter on the first hit.
    const quick = page.getByTestId('case-quick-add');
    await page.locator('[data-role="subject"]').click();
    await quick.fill(`${prefix} ${run} pump`);
    await expect(page.getByRole('option').first()).toContainText(`${prefix} ${run} pump P-12`);
    await quick.press('Enter');
    const members = page.getByTestId('case-members');
    await expect(members).toContainText(`${prefix} ${run} pump P-12`);
    await expect(quick).toHaveValue('');
    await page.locator('[data-role="context"]').click();
    await quick.fill(`${prefix} ${run} line`);
    await expect(page.getByRole('option').first()).toContainText(`${prefix} ${run} line L2`);
    await page.getByRole('option').first().click();
    await expect(members).toContainText(`${prefix} ${run} line L2`);
    await expect(members).toContainText(app.t('case-role-context'));
    await capture('case-members', { caption: 'Members grouped by role' });

    // A dated link between two members.
    const composer = page.getByTestId('case-link-composer');
    // Both memberships committed before reading their ids (slow under load).
    let memberIds: string[][] = [];
    await expect
      .poll(async () => {
        memberIds = (await api.post(`/cases/${caseId}/timeline`, {})).members.map(
          (m: any) => [m.source.title, m.source.id],
        );
        return memberIds.length;
      })
      .toBe(2);
    const id = (t: string) => memberIds.find(([title]: string[]) => title.includes(t))[1];
    await composer.getByTestId('case-link-src').selectOption(id('pump'));
    await composer.getByTestId('case-link-tgt').selectOption(id('line'));
    await composer.getByTestId('case-link-type').selectOption('feeds');
    await composer.getByTestId('case-link-since').fill('2026-10-01T08:30');
    await composer.getByTestId('case-link-submit').click();
    await app.expectToast(app.t('case-link-created'));

    // Timeline over a year: the dated link, the two memberships.
    const timeline = page.getByTestId('case-timeline');
    await timeline.locator('[data-range="8760"]').click();
    await expect(timeline.locator('[data-kind="link_open"]')).toHaveCount(3);
    await expect(timeline).toContainText('2026-10-01');
    await capture('case-timeline', { caption: 'Timeline: memberships and a dated link' });

    // Focus on the line: what touches it (its membership, the feeds link).
    await members.locator('[data-member]', { hasText: `${prefix} ${run} line L2` }).locator('button').first().click();
    await expect(timeline.locator('[data-kind="link_open"]')).toHaveCount(2);
    await expect(timeline.locator('[data-kind="change"]')).toHaveCount(1);
    await capture('case-focus', { caption: 'Focus on one member' });

    // Remove the line: closed, kept in the timeline.
    await members
      .locator('[data-member]', { hasText: `${prefix} ${run} line L2` })
      .getByTestId('case-remove-member')
      .click();
    await expect(members).not.toContainText(`${prefix} ${run} line L2`);
    await expect(timeline.locator('[data-kind="link_close"]')).toHaveCount(1);

    // Status in place.
    await page.locator('[data-status="concluded"]').click();
    await expect
      .poll(async () => (await api.get(`/ontology/objects/${caseId}`)).properties.status)
      .toBe('concluded');
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
    await form.getByTestId('add-to-case-role').selectOption('evidence');
    await form.getByRole('button', { name: app.t('case-add-submit'), exact: true }).click();
    await app.expectToast(app.t('case-added'));
    const tl = await api.post(`/cases/${c.id}/timeline`, {});
    expect(tl.members.map((m: any) => m.source.id)).toContain(pump.id);
  });

  test('a case opened from an event gets its window; ranges of members join the timeline', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump P-9`,
      properties: { serial: 'S-9', status: 'running' },
    });
    const c = await api.post('/cases', {
      title: `${prefix} alarm`,
      from: { kind: 'event', at: '2026-10-03T10:02:00Z' },
    });
    expect(c.properties.window_from).toMatch(/^2026-10-03T08:02:00/);
    await api.post('/ontology/links', {
      link_type: 'in_case',
      source_id: pump.id,
      target_id: c.id,
      attributes: { role: 'subject' },
    });
    await api.post('/time-ranges', {
      scope_kind: 'object',
      scope_id: pump.id,
      label: `${prefix} night shift`,
      planned_start: '2026-10-03T06:00:00Z',
      planned_end: '2026-10-03T14:00:00Z',
    });
    await app.goto(`/cases/${c.id}`);
    await expect(page.getByTestId('case-window-from')).toHaveValue('2026-10-03T08:02');
    const timeline = page.getByTestId('case-timeline');
    await expect(timeline.locator('[data-kind="range"]')).toContainText(`${prefix} night shift`);
  });

  test('replays the case at an instant: graph, places and the timeline cursor', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    await api.post('/ontology/packs/investigation/install', {});
    const obj = (type_key: string, title: string, properties = {}) =>
      api.post('/ontology/objects', { type_key, title: `${prefix} ${title}`, properties });
    const ann = await obj('person', 'Ann');
    const bob = await obj('person', 'Bob');
    const ev = await obj('event', 'break-in', { location: { lat: 45.76, lon: 4.83 } });
    const flat = await obj('place', 'flat', { location: { lat: 45.75, lon: 4.85 } });
    const c = await obj('case', 'theft', { status: 'open' });
    const start = new Date(Date.now() - 3 * 3600_000).toISOString();
    for (const o of [ann, bob, ev]) {
      await api.post('/ontology/links', {
        link_type: 'in_case',
        source_id: o.id,
        target_id: c.id,
        attributes: { role: 'actor' },
        valid_from: start,
      });
    }
    const knows = await api.post('/ontology/links', {
      link_type: 'knows', source_id: ann.id, target_id: bob.id, valid_from: start,
    });
    const lives = await api.post('/ontology/links', {
      link_type: 'located_at', source_id: ann.id, target_id: flat.id,
      attributes: { kind: 'lives' }, valid_from: start,
    });
    for (const l of [knows, lives]) await api.post(`/ontology/links/${l.id}/close`, {});

    await app.goto(`/cases/${c.id}`);
    const snap = page.getByTestId('case-snapshot');
    const nodes = snap.getByTestId('ontology-graph').locator('g.cursor-pointer');
    // Now: three members, the links are closed, only the event is placed.
    await expect(nodes).toHaveCount(3);
    await expect(snap.getByTestId('ontology-graph').locator('line')).toHaveCount(0);
    await expect(snap.getByTestId('case-map')).toBeVisible();

    // Step back to the last link opening: Ann knows Bob and lives at the flat.
    const timeline = page.getByTestId('case-timeline');
    await timeline.locator('[data-kind="link_open"]').last().locator('button').first().click();
    await expect(snap.getByTestId('ontology-graph').locator('line')).toHaveCount(1);
    // Ann is placed through her located_at link, open at that instant.
    await expect(snap.getByTestId('case-map')).toContainText(`${prefix} Ann (${prefix} flat)`);
    await expect(timeline.locator('li.opacity-40').first()).toBeVisible();
    await capture('case-snapshot', { caption: 'The case at an instant: graph and places' });

    // A graph node focuses its lane; Now returns to the current state.
    await nodes.filter({ hasText: `${prefix} Bob` }).click();
    await expect(page.getByTestId('case-timeline')).toContainText(`${prefix} Bob`);
    await snap.getByRole('button', { name: app.t('case-snapshot-now'), exact: true }).click();
    await expect(snap.getByTestId('ontology-graph').locator('line')).toHaveCount(0);
  });

  test('starts a page from a template, designates it, freezes it at the conclusion', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    await api.post('/ontology/packs/investigation/install', {});
    const run = Date.now().toString(36);
    const c = await api.post('/ontology/objects', {
      type_key: 'case', title: `${prefix} trip ${run}`, properties: { status: 'open' },
    });
    await app.goto(`/cases/${c.id}`);
    const panel = page.getByTestId('case-pages');
    await expect(panel.getByTestId('case-page-template').locator('option[value="five_whys"]')).toHaveCount(1);
    await panel.getByTestId('case-page-template').selectOption('five_whys');
    await panel.getByTestId('case-page-title').fill(`${prefix} why ${run}`);
    await panel.getByTestId('case-page-title').press('Enter');

    // The page opens filled with the template's sections.
    const editor = new DocEditor(app, page);
    await editor.waitSynced();
    await expect(editor.content.locator('h2').first()).toHaveText('Problem');
    await expect(editor.content.locator('ol li')).toHaveCount(5);
    await capture('case-page-template', { caption: 'A 5 whys page started from the case' });
    const docId = editor.id;

    await app.goto(`/cases/${c.id}`);
    await panel.getByTestId('case-page-row').filter({ hasText: `${prefix} why ${run}` })
      .getByTestId('case-page-designate').click();
    await expect(panel.getByTestId('case-page-conclusion')).toBeVisible();
    await capture('case-pages', { caption: 'Case pages with the conclusion designated' });

    // Concluding the case freezes the conclusion page in a named version.
    await page.getByTestId('case-header').locator('[data-status="concluded"]').click();
    await expect.poll(async () => JSON.stringify(await api.get(`/docs/${docId}/versions`)))
      .toContain('"Conclusion"');
  });

  test('an archived case proposes to purge the persons only it holds', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    await api.post('/ontology/packs/investigation/install', {});
    const run = Date.now().toString(36);
    const obj = (type_key: string, title: string, properties = {}) =>
      api.post('/ontology/objects', { type_key, title: `${prefix} ${title} ${run}`, properties });
    const c = await obj('case', 'fraud', { status: 'open' });
    const ann = await obj('person', 'Ann');
    const bob = await obj('person', 'Bob');
    const carl = await obj('person', 'Carl');
    for (const o of [ann, bob]) {
      await api.post('/ontology/links', {
        link_type: 'in_case', source_id: o.id, target_id: c.id, attributes: { role: 'actor' },
      });
    }
    // Bob is also known outside the case: he is not proposed.
    await api.post('/ontology/links', { link_type: 'knows', source_id: bob.id, target_id: carl.id });

    await app.goto(`/cases/${c.id}`);
    await expect(page.getByTestId('case-purge')).toHaveCount(0);
    await page.getByTestId('case-header').locator('[data-status="archived"]').click();
    const panel = page.getByTestId('case-purge');
    await expect(panel).toContainText(`${prefix} Ann ${run}`);
    await expect(panel).not.toContainText(`${prefix} Bob ${run}`);
    await capture('case-purge', { caption: 'Personal data proposed for purge at archiving' });

    await panel.getByTestId('case-purge-start').click();
    await page.getByRole('alertdialog').getByRole('button').last().click();
    await expect(page.getByTestId('case-purge')).toHaveCount(0);
    await expect.poll(() => api.get(`/ontology/objects/${ann.id}`).then(() => 'kept', () => 'purged'))
      .toBe('purged');
    expect(await api.get(`/ontology/objects/${bob.id}`)).toHaveProperty('id', bob.id);
  });
});
