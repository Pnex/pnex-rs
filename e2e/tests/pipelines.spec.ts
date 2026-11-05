// Data pipelines without YAML: a table loaded from a CSV, a pipeline made on the
// screen, edited, refused when its code is wrong, then run by the real worker when
// the stack has one (`task pipeline-worker:up`, PNEX_E2E_PIPELINE_WORKER=1).
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { expect, test } from '../src/fixtures.ts';
import { BASE_URL } from '../src/env.ts';
import { FlowsPage } from '../src/pages/flows.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';
import { memoryKey } from '../src/scenarios.ts';

const WORKER = process.env.PNEX_E2E_PIPELINE_WORKER === '1';
/** The admin CLI to try for real, built from this checkout. */
const CLI = process.env.PNEX_E2E_CLI;

test.describe('pipelines', { tag: '@pipelines' }, () => {
  test('a table and a pipeline, from the screen', async ({ app, page, api, prefix }) => {
    test.setTimeout(240_000);
    const slug = prefix.replace(/[^a-zA-Z0-9]/g, '_').toLowerCase();
    const raw = `${slug}_raw`;
    const clean = `${slug}_clean`;
    const name = `${prefix} pipeline`;
    const main = page.getByRole('main');

    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-tab-tables') }).click();
    await main.getByRole('button', { name: app.t('pipe-table-add') }).click();
    const newTable = dialog(page, app.t('pipe-table-add'));
    await newTable.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(raw);
    await newTable.getByRole('button', { name: app.t('pipe-create'), exact: true }).click();
    await expect(page).toHaveURL(/\/pipelines\/tables\//);

    // A CSV becomes the first version, with its rows on the page.
    await main.getByTestId('table-upload').setInputFiles({
      name: 'raw.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,city\n1,Lyon\n2,Nantes\n3,Lyon\n'),
    });
    await expect(main.getByTestId('table-sample')).toContainText('Nantes');
    // Fluent isolates its placeables: the number and the words are matched apart.
    await expect(main.getByTestId('table-sample')).toContainText(/100\W+% filled|100\W+% rempli/);
    await expect(main.getByRole('listitem').filter({ hasText: app.t('pipe-version-committed') })).toHaveCount(1);

    // A second version replaces the rows; the first can still be read.
    await main.getByTestId('table-upload').setInputFiles({
      name: 'second.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,city\n9,Brest\n'),
    });
    await expect(main.getByTestId('table-sample')).toContainText('Brest');
    await expect(main.getByTestId('table-sample')).not.toContainText('Nantes');
    await main.getByRole('button', { name: app.t('pipe-version-read') }).last().click();
    await expect(main.getByTestId('table-sample')).toContainText('Nantes');
    await main.getByRole('button', { name: app.t('pipe-table-back-current') }).click();
    await expect(main.getByTestId('table-sample')).toContainText('Brest');
    await main.getByTestId('table-upload').setInputFiles({
      name: 'third.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,city\n1,Lyon\n2,Nantes\n3,Lyon\n'),
    });
    await expect(main.getByTestId('table-sample')).toContainText('Nantes');

    // A file that cannot be read is refused, in words, and changes nothing.
    await main.getByTestId('table-upload').setInputFiles({
      name: 'bad.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,city\n1,Lyon,extra\n'),
    });
    await expect(main).toContainText(app.t('err-dataset-csv-invalid'));
    await expect(main.getByTestId('table-sample')).toContainText('Nantes');

    // How fresh it must stay.
    const field = main.getByLabel(app.t('pipe-table-freshness'), { exact: true });
    await field.fill('48');
    await field.locator('..').getByRole('button', { name: app.t('pipe-save') }).click();
    await expect
      .poll(async () => (await api.list<any>('/datasets')).find((t) => t.name === raw).max_age_hours)
      .toBe(48);

    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-add') }).click();
    const create = dialog(page, app.t('pipe-add'));
    await create.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await create.getByLabel(app.t('pipe-source'), { exact: true }).selectOption(raw);
    await create.getByLabel(app.t('pipe-output'), { exact: true }).fill(clean);
    await create.getByRole('button', { name: app.t('pipe-create'), exact: true }).click();
    await expect(page).toHaveURL(/\/pipelines\/[0-9a-f-]{36}$/);

    // The graph reads left to right, one node per table.
    await expect(page.getByTestId(`table-node-${raw}`)).toBeVisible();
    await page.getByTestId(`table-node-${clean}`).click();

    // A query edited on the screen is saved as a new version.
    const sql = 'SELECT city, count(*) AS n FROM source GROUP BY city';
    await page.getByLabel(app.t('pipe-sql'), { exact: true }).fill(sql);
    await page.getByText(app.t('pipe-checks'), { exact: true }).click();
    await page.getByLabel(app.t('pipe-check-min-rows'), { exact: true }).fill('1');
    await page.getByLabel(app.t('pipe-check-min-rows'), { exact: true }).blur();
    await page.getByTestId('pipe-save').click();
    await expect(page.getByTestId('pipe-save')).toBeDisabled();

    const saved = (await api.list<any>('/pipelines')).find((p) => p.name === name);
    expect(saved.version).toBe(2);
    expect(saved.document).toContain(sql);
    expect(saved.document).toContain('min_rows');

    // The code shows the same pipeline; a wrong one is named and locks the way out.
    await page.getByTestId('pipe-tab-code').click();
    await expect(page.getByTestId('pipe-yaml')).toHaveValue(/GROUP BY city/);
    await page.getByTestId('pipe-yaml').fill('transforms: []');
    await expect(page.getByTestId('pipe-violations')).toBeVisible();
    await expect(page.getByTestId('pipe-save')).toBeDisabled();
    await page.getByRole('button', { name: app.t('pipe-tab-runs') }).click();
    await expect(page.getByTestId('pipe-message')).toHaveText(app.t('pipe-yaml-fix-first'));
    await page.getByTestId('pipe-yaml').fill(saved.document);
    await expect(page.getByTestId('pipe-violations')).toBeHidden();

    if (WORKER) {
      await page.reload();
      await page.getByTestId('pipe-run').click();
      await expect(page.getByTestId('build-steps')).toContainText(app.t('pipe-step-done'), { timeout: 60_000 });
      await page.getByTestId(`table-node-${clean}`).click();
      await expect(page.getByTestId('table-sample')).toContainText('Nantes');

      // What depends on another pipeline shows what runs first, then runs both in order.
      const finalTable = `${slug}_final`;
      const next = await api.post<any>('/pipelines', {
        name: `${prefix} next`,
        document: `transforms:\n  - output: ${finalTable}\n    inputs: {c: ${clean}}\n    sql: SELECT * FROM c\n`,
      });
      await app.goto(`/pipelines/${next.id}`);
      await page.getByTestId('pipe-run').click();
      const plan = dialog(page, app.t('pipe-plan-title'));
      await expect(plan.getByRole('listitem')).toHaveCount(2);
      await expect(plan.getByRole('listitem').first()).toContainText(name);
      await page.getByTestId('pipe-plan-confirm').click();
      await expect(plan).toBeHidden();
      await expect(page.getByTestId('build-steps').getByRole('listitem').last()).toContainText(app.t('pipe-step-done'), {
        timeout: 90_000,
      });

      // The lineage written by the runs, on the page of the table in the middle.
      const middle = (await api.list<any>('/datasets')).find((t) => t.name === clean);
      await app.goto(`/pipelines/tables/${middle.id}`);
      await expect(page.getByTestId(`table-node-${raw}`)).toBeVisible();
      await expect(page.getByTestId(`table-node-${finalTable}`)).toBeVisible();
      await page.getByTestId(`table-node-${finalTable}`).click();
      await expect(page).not.toHaveURL(new RegExp(middle.id));

      await api.delete(`/pipelines/${next.id}`);
      await api.delete(`/datasets/${(await api.list<any>('/datasets')).find((t) => t.name === finalTable).id}`);
      await app.goto(`/pipelines/${saved.id}`);
    }

    // A schedule set on the screen is the schedule of the pipeline, and can be removed.
    await page.getByRole('button', { name: app.t('pipe-tab-schedule') }).click();
    await page.getByLabel(app.t('pipe-tab-schedule'), { exact: true }).selectOption('day');
    await expect
      .poll(async () => (await api.get<any>(`/pipelines/${saved.id}`)).recurrence?.unit)
      .toBe('day');
    await expect(page.getByText(app.t('pipe-next-run').split('{')[0].trim())).toBeVisible();
    await page.getByLabel(app.t('pipe-tab-schedule'), { exact: true }).selectOption('');
    await expect.poll(async () => (await api.get<any>(`/pipelines/${saved.id}`)).recurrence ?? null).toBeNull();
    await page.getByRole('button', { name: app.t('pipe-tab-transform') }).click();

    if (WORKER) {
      // Chained runs are listed, each with its steps.
      await page.getByRole('button', { name: app.t('pipe-tab-runs') }).click();
      await expect(page.getByTestId('pipe-runs')).toContainText(app.t('pipe-run-done'));
      await app.goto('/pipelines');
      await main.getByRole('button', { name: app.t('pipe-tab-builds') }).click();
      await main.locator('tr').filter({ hasText: name }).first().click();
      await expect(dialog(page, name).getByTestId('build-steps')).toContainText(app.t('pipe-step-done'));
      await dialog(page, name).getByRole('button', { name: app.t('common-close') }).last().click();
      await app.goto(`/pipelines/${saved.id}`);
    }

    // An earlier version comes back as a new one.
    await page.getByRole('button', { name: app.t('web-sites-versions') }).click();
    await page.getByRole('button', { name: app.t('web-sites-version-restore') }).click();
    await expect.poll(async () => (await api.get<any>(`/pipelines/${saved.id}`)).version).toBe(3);
    expect((await api.get<any>(`/pipelines/${saved.id}`)).document).toContain('SELECT * FROM source');

    // Deleted from the screen; a computed table stays.
    await page.reload();
    await page.getByRole('button', { name: app.t('pipe-tab-schedule') }).click();
    await page.getByRole('button', { name: app.t('pipe-delete-pipeline') }).click();
    await confirmDialog(page, app.t('pipe-delete-pipeline')).getByRole('button', { name: app.t('pipe-delete') }).click();
    await expect(page).toHaveURL(/\/pipelines$/);
    await expect(main.locator('tr').filter({ hasText: name })).toHaveCount(0);

    for (const t of await api.list<any>('/datasets')) {
      if (t.name === raw || t.name === clean) await api.delete(`/datasets/${t.id}`);
    }
  });

  test('a file over the limit of one request goes through in parts', async ({ app, page, api, prefix }) => {
    test.setTimeout(240_000);
    const table = await api.post<{ id: string }>('/datasets', { name: `${prefix}-parts` });
    const main = page.getByRole('main');
    const parts: number[] = [];
    page.on('request', (request) => {
      if (/\/uploads\/[^/]+\/parts\/\d+$/.test(request.url())) {
        parts.push(request.postDataBuffer()?.length ?? 0);
      }
    });
    await app.goto(`/pipelines/tables/${table.id}`);
    // 45 MiB, what Playwright takes as a buffer: over one part of 32 MiB, so two requests.
    const rows = 1_200_000;
    const lines = Array.from({ length: rows }, (_, i) => `${i},City of ${i} with a longer name`);
    await main.getByTestId('table-upload').setInputFiles({
      name: 'large.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from(`id,city\n${lines.join('\n')}\n`),
    });
    await expect(main.getByTestId('table-sample')).toContainText('City of', { timeout: 180_000 });
    expect(parts.length).toBe(2);
    for (const size of parts) expect(size).toBeLessThanOrEqual(64 * 1024 * 1024);
    const done = await api.get<{ rows: number }>(`/datasets/${table.id}`);
    expect(done.rows).toBe(rows);
    await api.delete(`/datasets/${table.id}`);
  });

  test('on a phone, the graph and the details share the screen', async ({ app, page, api, prefix }) => {
    await page.setViewportSize({ width: 390, height: 800 });
    const slug = prefix.replace(/[^a-zA-Z0-9]/g, '_').toLowerCase();
    const table = await api.post<any>('/datasets', { name: `${slug}_phone` });
    const made = await api.post<any>('/pipelines', {
      name: `${prefix} phone`,
      document: `transforms:\n  - output: ${slug}_out\n    inputs: {s: ${slug}_phone}\n    sql: SELECT * FROM s\n`,
    });
    await app.goto(`/pipelines/${made.id}`);
    await page.getByTestId(`table-node-${slug}_out`).click();
    await expect(page.getByTestId('pipe-transform')).toBeVisible();
    await expect(page.getByTestId('pipe-save')).toBeInViewport();
    await api.delete(`/pipelines/${made.id}`);
    await api.delete(`/datasets/${table.id}`);
  });

  test('a connector keeps its password out of the screen', async ({ app, page, api, prefix }) => {
    const name = `${prefix} crm`;
    const password = `pw-${prefix}`;
    const main = page.getByRole('main');
    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-tab-connectors') }).click();
    await main.getByRole('button', { name: app.t('pipe-connector-add') }).click();
    const form = dialog(page, app.t('pipe-connector-add'));
    await form.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await form.getByLabel(app.t('pipe-connector-host'), { exact: true }).fill('db.example.invalid');
    await form.getByLabel(app.t('pipe-connector-database'), { exact: true }).fill('crm');
    await form.getByLabel(app.t('pipe-connector-username'), { exact: true }).fill('reader');
    await form.getByRole('textbox', { name: app.t('pipe-connector-password') }).fill(password);
    await form.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(form).toBeHidden();

    const row = main.locator('tr').filter({ hasText: name });
    await expect(row).toContainText('db.example.invalid:5432/crm');
    await expect(main).not.toContainText(password);
    const saved = (await api.list<any>('/connectors')).find((c) => c.name === name);
    expect(JSON.stringify(saved)).not.toContain(password);
    expect(saved.has_password).toBe(true);

    // The test says it failed, without text from the other side.
    await row.getByRole('button', { name: app.t('pipe-connector-test') }).click();
    await expect(page.getByText(new RegExp(`${app.t('err-connector-unreachable').slice(0, 20)}|${app.t('err-connector-host-refused').slice(0, 20)}`)).first()).toBeVisible();

    // Edited without its password: the one in the vault stays.
    await row.click();
    const edit = dialog(page, app.t('pipe-connector-edit'));
    await edit.getByLabel(app.t('pipe-connector-database'), { exact: true }).fill('crm2');
    await edit.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(edit).toBeHidden();
    await expect(row).toContainText('db.example.invalid:5432/crm2');
    const kept = (await api.list<any>('/connectors')).find((c) => c.name === name);
    expect(kept.has_password).toBe(true);

    await row.getByRole('button', { name: app.t('pipe-delete') }).click();
    await confirmDialog(page, app.t('pipe-connector-delete-title')).getByRole('button', { name: app.t('pipe-delete') }).click();
    await expect(row).toHaveCount(0);
  });

  test('a new version of a table reaches a deployed flow', async ({ app, api, prefix }) => {
    test.setTimeout(180_000);
    const slug = prefix.replace(/[^a-zA-Z0-9]/g, '_').toLowerCase();
    const tableName = `${slug}_events`;
    const key = memoryKey(prefix, 'table-event');
    const table = await api.post<any>('/datasets', { name: tableName });

    const flows = new FlowsPage(app);
    await flows.open();
    const name = `${prefix} table flow`;
    const editor = await flows.create(name);
    await editor.addNode('flows-palette-dataset-trigger');
    await editor.field(app.t('flows-dataset-trigger-table')).fill(tableName);
    await editor.field(app.t('flows-dataset-trigger-table')).blur();
    await editor.move(app.t('flows-palette-dataset-trigger'), 300, 300);
    await editor.addNode('flows-palette-memory-write');
    await editor.inspector.getByRole('textbox', { name: new RegExp(`^${app.t('flows-memory-key')}\\b`) }).fill(key);
    await editor.move(app.t('flows-palette-memory-write'), 600, 300);
    await editor.closeInspector();
    await editor.wire(app.t('flows-palette-dataset-trigger'), 0, app.t('flows-palette-memory-write'));
    await editor.save();
    await editor.deploy();

    // A frame sent before the flow listens is gone by design: load until one arrives.
    let n = 0;
    await expect
      .poll(
        async () => {
          n += 1;
          await api.upload(`/datasets/${table.id}/transactions/csv`, Buffer.from(`a\n${n}\n`), 'text/csv');
          // Only this trigger writes this key; the query answers numbers.
          const res = await api.post('/memory/values/query', { refs: [{ key, field: 'ts_ms' }] });
          return res.results[0]?.value ?? 0;
        },
        { timeout: 120_000, intervals: [15_000], message: 'dataset event in the flow' },
      )
      .toBeGreaterThan(0);

    await editor.stop();
    await flows.open();
    await flows.delete(name);
    await api.delete(`/datasets/${table.id}`);
  });

  test('the CLI follows a run and reports how it ended', async ({ api, tokens, runState, prefix }) => {
    test.skip(!CLI, 'PNEX_E2E_CLI not set');
    test.setTimeout(120_000);
    const slug = prefix.replace(/[^a-zA-Z0-9]/g, '_').toLowerCase();
    const typeKey = `${slug}_thing`.slice(0, 40);
    await api.post('/ontology/types', { def: { key: typeKey, name: typeKey, properties: [{ key: 'code', name: 'Code', kind: 'text', max_len: 16, required: true, unique: true }] } });
    const table = await api.post<any>('/datasets', { name: `${slug}_cli` });
    await api.upload(`/datasets/${table.id}/transactions/csv`, Buffer.from('code\nA\nB\n'), 'text/csv');
    const mapping = `key: ${slug}_cli\nname: CLI\nobjects:\n  - as: t\n    type: ${typeKey}\n    key: code\n    title: { from: code }\n    properties:\n      code: { from: code }\n`;
    const imp = await api.post<any>('/ontology/imports', { name: `${prefix} cli`, mapping, source: 'dataset', source_url: table.id, mode: 'incremental' });

    // A session of its own, gone with the test.
    const home = mkdtempSync(path.join(tmpdir(), 'pnex-cli-'));
    mkdirSync(path.join(home, 'pnex'));
    writeFileSync(
      path.join(home, 'pnex', 'session.json'),
      JSON.stringify({ url: BASE_URL, org: runState.orgId, access_token: tokens.access, refresh_token: 'unused', expires_at: Math.floor(Date.now() / 1000) + 3600 }),
      { mode: 0o600 },
    );
    const pnex = (...args: string[]) =>
      spawnSync(CLI!, args, { env: { ...process.env, XDG_CONFIG_HOME: home }, encoding: 'utf8', timeout: 90_000 });
    try {
      expect(pnex('import', 'run', imp.id, '--wait').stderr).toContain('--yes');

      const ok = pnex('import', 'run', imp.id, '--wait', '--yes');
      expect(ok.status, ok.stderr).toBe(0);
      expect(JSON.parse(ok.stdout).state).toBe('applied');

      // Its table gone, the run fails: the exit code and the message say so.
      await api.delete(`/datasets/${table.id}`);
      const bad = pnex('import', 'run', imp.id, '--wait', '--yes');
      expect(bad.status).toBe(1);
      expect(bad.stderr).toContain('dataset-not-found');
    } finally {
      rmSync(home, { recursive: true, force: true });
      await api.delete(`/ontology/imports/${imp.id}`);
    }
  });

  test('the CLI loads a file over what a proxy lets through in one request', async ({ api, tokens, runState, prefix }) => {
    test.skip(!CLI, 'PNEX_E2E_CLI not set');
    test.setTimeout(300_000);
    const table = await api.post<any>('/datasets', { name: `${prefix}-cli-load` });
    const home = mkdtempSync(path.join(tmpdir(), 'pnex-cli-'));
    mkdirSync(path.join(home, 'pnex'));
    writeFileSync(
      path.join(home, 'pnex', 'session.json'),
      JSON.stringify({ url: BASE_URL, org: runState.orgId, access_token: tokens.access, refresh_token: 'unused', expires_at: Math.floor(Date.now() / 1000) + 3600 }),
      { mode: 0o600 },
    );
    const pnex = (...args: string[]) =>
      spawnSync(CLI!, args, { env: { ...process.env, XDG_CONFIG_HOME: home }, encoding: 'utf8', timeout: 240_000 });
    // 120 MB: one request of this size is refused by Cloudflare.
    const rows = 3_000_000;
    const file = path.join(home, 'large.csv');
    const line = (i: number) => `${i};City of ${i} with a longer name\n`;
    const chunks = ['id;city\n'];
    for (let i = 0; i < rows; i++) chunks.push(line(i));
    writeFileSync(file, chunks.join(''));
    try {
      expect(pnex('dataset', 'load', table.id, file).stderr).toContain('--yes');

      const ok = pnex('dataset', 'load', table.id, file, '--delimiter', ';', '--yes');
      expect(ok.status, ok.stderr).toBe(0);
      expect(JSON.parse(ok.stdout).state).toBe('done');
      expect(JSON.parse(ok.stdout).parts).toBeGreaterThan(1);
      expect((await api.get<any>(`/datasets/${table.id}`)).rows).toBe(rows);

      // A file that does not read: the exit code and the message say so.
      const broken = path.join(home, 'broken.csv');
      writeFileSync(broken, 'a,b\n1,2,3\n');
      const bad = pnex('dataset', 'load', table.id, broken, '--delimiter', ',', '--yes');
      expect(bad.status).toBe(1);
      expect(bad.stderr).toContain('dataset-csv-invalid');
    } finally {
      rmSync(home, { recursive: true, force: true });
      await api.delete(`/datasets/${table.id}`);
    }
  });
});

test.describe('pipelines in French', { tag: ['@pipelines', '@i18n'] }, () => {
  test('the studio and the page of a table render', async ({ app, page, api, prefix, t }) => {
    const slug = prefix.replace(/[^a-zA-Z0-9]/g, '_').toLowerCase();
    const table = await api.post<any>('/datasets', { name: `${slug}_i18n` });
    await api.upload(`/datasets/${table.id}/transactions/csv`, Buffer.from('a,b\n1,\n'), 'text/csv');
    const made = await api.post<any>('/pipelines', {
      name: `${prefix} i18n`,
      document: `transforms:\n  - output: ${slug}_i18n_out\n    inputs: {s: ${slug}_i18n}\n    sql: SELECT * FROM s\n`,
    });
    await app.goto(`/pipelines/tables/${table.id}`);
    await expect(page.getByRole('main')).toContainText(t('pipe-table-versions'));
    await expect(page.getByTestId('table-sample')).toBeVisible();
    await app.goto(`/pipelines/${made.id}`);
    await page.getByTestId(`table-node-${slug}_i18n_out`).click();
    await expect(page.getByLabel(t('pipe-sql'), { exact: true })).toBeVisible();
    for (const tab of ['pipe-tab-code', 'pipe-tab-runs', 'pipe-tab-schedule']) {
      await page.getByRole('button', { name: t(tab) }).click();
    }
    await api.delete(`/pipelines/${made.id}`);
    await api.delete(`/datasets/${table.id}`);
  });
});
