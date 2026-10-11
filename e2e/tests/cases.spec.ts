// Cases (cases.md K1, K2, K6): a case opened from one line, members added
// as you type with their role, a dated link between two members, the
// timeline (membership and link events, focus on a member), removal kept
// as history, status changed in place. Objects are titled with the test
// prefix: the ontology is not swept between runs.
import { expect, test } from '../src/fixtures.ts';
import { dialog } from '../src/pages/shell.ts';

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
    await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump P-12`,
      properties: { serial: 'S-12', status: 'running' },
    });
    await api.post('/ontology/objects', {
      type_key: 'line',
      title: `${prefix} line L2`,
      properties: { code: 'L2' },
    });

    // One line + Enter opens the case and lands in its workspace.
    await app.goto('/cases');
    await capture('cases-list', { caption: 'Cases: open one from a single line' });
    await page.getByTestId('case-new-title').fill(`${prefix} low pressure`);
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
    await quick.fill(`${prefix} pump`);
    await expect(page.getByRole('option').first()).toContainText(`${prefix} pump P-12`);
    await quick.press('Enter');
    const members = page.getByTestId('case-members');
    await expect(members).toContainText(`${prefix} pump P-12`);
    await expect(quick).toHaveValue('');
    await page.locator('[data-role="context"]').click();
    await quick.fill(`${prefix} line`);
    await expect(page.getByRole('option').first()).toContainText(`${prefix} line L2`);
    await page.getByRole('option').first().click();
    await expect(members).toContainText(`${prefix} line L2`);
    await expect(members).toContainText(app.t('case-role-context'));
    await capture('case-members', { caption: 'Members grouped by role' });

    // A dated link between two members.
    const composer = page.getByTestId('case-link-composer');
    const memberIds = (await api.post(`/cases/${caseId}/timeline`, {})).members.map(
      (m: any) => [m.source.title, m.source.id],
    );
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
    await members.locator('[data-member]', { hasText: `${prefix} line L2` }).locator('button').first().click();
    await expect(timeline.locator('[data-kind="link_open"]')).toHaveCount(2);
    await expect(timeline.locator('[data-kind="change"]')).toHaveCount(1);
    await capture('case-focus', { caption: 'Focus on one member' });

    // Remove the line: closed, kept in the timeline.
    await members
      .locator('[data-member]', { hasText: `${prefix} line L2` })
      .getByTestId('case-remove-member')
      .click();
    await expect(members).not.toContainText(`${prefix} line L2`);
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
});
