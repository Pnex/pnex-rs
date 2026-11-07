// Agents page (agent-environments.md D757, D761): create an agent, start a run by giving
// its input file, follow it to its end, download its output, stop another run. A runner
// stands in through its own API (claim, transfer, logs, finish): the sandbox itself is
// proven on a cluster (docs/architecture/agent-environments-recette.md).
import type { APIRequestContext } from '@playwright/test';
import { expect, test } from '../src/fixtures.ts';
import { BASE_URL } from '../src/env.ts';
import { dialog, fieldAfterLabel } from '../src/pages/shell.ts';

const DIGEST = `sha256:${'ab'.repeat(32)}`;

/** A runner of the org, driven by hand. */
async function enrol(api: any, request: APIRequestContext, name: string): Promise<string> {
  const join = await api.post('/runners/join-tokens', { name });
  const res = await request.post(`${BASE_URL}/api/v1/runner/register`, {
    data: { join_token: join.token, capabilities: ['sandbox:pod', 'builder:buildkit'] },
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  return (await res.json()).token;
}

async function runner(request: APIRequestContext, token: string, method: string, path: string, data?: unknown) {
  return request.fetch(`${BASE_URL}/api/v1/runner/${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}` },
    data,
  });
}

test.describe('agents', { tag: '@agents' }, () => {
  test('create, run with a file, follow, download, stop', async ({ app, page, api, prefix, capture, request }) => {
    test.setTimeout(120_000);
    // Unique per execution: a test that failed half way leaves its objects behind.
    const slug = `${prefix.toLowerCase().replace(/[^a-z0-9-]/g, '-').slice(0, 32)}-${Date.now() % 1_000_000}`;
    const main = page.getByRole('main');
    const tab = (key: string) => main.getByRole('tab', { name: app.t(key) });

    // An environment with an active image, built by a runner we drive.
    const registry = await api.post('/image-registries', {
      name: `${slug}-reg`,
      host: 'registry.example.org',
      repository: `team/${slug}`,
      tls: true,
      write_username: 'ci',
      write_secret: { value: 'registry-write-value' },
    });
    const env = await api.post('/environments', { name: `${slug}-env`, registry_id: registry.id });
    await api.post(`/environments/${env.id}/versions`, { dockerfile: 'FROM scratch' });
    const token = await enrol(api, request, `${slug}-runner`);
    await api.post(`/environments/${env.id}/versions/1/builds`);
    let res = await runner(request, token, 'POST', 'jobs/claim');
    const build = await res.json();
    expect(build.kind).toBe('environment_build');
    res = await runner(request, token, 'POST', `jobs/${build.id}/finish`, {
      outcome: 'succeeded',
      exit_code: 0,
      image_digest: DIGEST,
    });
    expect(res.status()).toBe(204);
    const builds = await api.get(`/environments/${env.id}/builds?limit=1&offset=0`);
    const current = await api.get(`/environments/${env.id}`);
    await api.post(`/environments/${env.id}/builds/${builds.results[0].id}/activate`, {
      expected_version: current.version,
    });

    // Create the agent from the page.
    await app.goto('/agents');
    await expect(main.locator('h1')).toHaveText(app.t('agents-title'));
    await main.getByRole('button', { name: app.t('agents-new') }).click();
    let dlg = dialog(page, app.t('agents-new'));
    const agentName = `${slug} bugfix`;
    await fieldAfterLabel(dlg, app.t('agents-field-name')).fill(agentName);
    await dlg.locator('#agent-env').selectOption({ label: `${slug}-env` });
    await fieldAfterLabel(dlg, app.t('agents-field-inputs')).fill('ticket');
    await fieldAfterLabel(dlg, app.t('agents-field-required-outputs')).fill('patch');
    await dlg.getByRole('button', { name: app.t('common-create'), exact: true }).click();
    await expect(dlg).toBeHidden();
    const row = main.locator('tr').filter({ hasText: agentName });
    await expect(row).toBeVisible();
    await expect(row).toContainText(app.t('agents-role-viewer'));
    await capture('agents-list', { caption: 'Agents: an agent and the files it takes and leaves' });

    // Its record opens on the editor: a shorter timeout saved makes version 2.
    await row.getByRole('button', { name: app.t('agents-edit-open') }).click();
    const timeout = main.locator('#agent-edit-timeout');
    await expect(timeout).toHaveValue('1800');
    await timeout.fill('1200');
    await main.getByRole('button', { name: app.t('agents-edit-save') }).click();
    await expect(main.getByText('v2', { exact: true })).toBeVisible();
    await capture('agents-editor', { caption: 'Agents: the record of an agent, edited and versioned' });
    await main.getByRole('button', { name: app.t('agents-edit-back') }).click();
    await expect(row).toContainText('v2');

    // Start a run, giving the ticket.
    await row.getByRole('button', { name: app.t('agents-start') }).click();
    dlg = page.getByRole('dialog');
    await dlg.locator('input[type=file]').setInputFiles({
      name: 'ticket.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('the parser drops the last field'),
    });
    await dlg.getByRole('button', { name: app.t('agents-start'), exact: true }).click();
    await expect(dlg).toBeHidden();

    // The runner takes it, reads the ticket, leaves the patch and its log, ends well.
    res = await runner(request, token, 'POST', 'jobs/claim');
    const job = await res.json();
    expect(job.kind).toBe('sandbox');
    const transfer = job.sandbox.artifacts.token;
    const url = job.sandbox.artifacts.url.replace(/^https?:\/\/[^/]+/, BASE_URL);
    res = await request.get(`${url}/inputs/ticket`, { headers: { Authorization: `Bearer ${transfer}` } });
    expect(await res.text()).toBe('the parser drops the last field');
    res = await request.put(`${url}/outputs/patch`, {
      headers: { Authorization: `Bearer ${transfer}`, 'x-pnex-artifact-form': 'file' },
      data: Buffer.from('--- a/parser.rs\n+++ b/parser.rs\n'),
    });
    expect(res.ok(), await res.text()).toBeTruthy();
    await runner(request, token, 'POST', `jobs/${job.id}/logs`, {
      containers: [{ container: 'job', text: 'agent: fixing the parser\nagent: done\n' }],
    });
    res = await runner(request, token, 'POST', `jobs/${job.id}/finish`, { outcome: 'succeeded', exit_code: 0 });
    expect(res.status()).toBe(204);

    // Followed on the page: its output downloads, its log reads.
    await tab('agents-tab-runs').click();
    const runRow = main.locator('tr').filter({ hasText: app.t('agents-run-succeeded') }).first();
    await expect(runRow).toBeVisible();
    await runRow.getByRole('button', { name: app.t('agents-open-run') }).click();
    const detail = page.getByRole('dialog');
    await expect(detail).toContainText('patch');
    await expect(detail).toContainText(app.t('agents-output-ready'));
    const download = page.waitForEvent('download');
    await detail.getByRole('button', { name: app.t('agents-download') }).click();
    // The browser may add an extension to a blob without one: the name stays the output's.
    expect((await download).suggestedFilename()).toMatch(/^patch(\.txt)?$/);
    await capture('agents-run-detail', { caption: 'Agents: a run, its outputs and its log' });
    await detail.getByRole('button', { name: app.t('common-close') }).click();

    // A second run, stopped from the page before it ends.
    await tab('agents-tab-agents').click();
    await row.getByRole('button', { name: app.t('agents-start') }).click();
    dlg = page.getByRole('dialog');
    await dlg.locator('input[type=file]').setInputFiles({
      name: 'ticket.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('second'),
    });
    await dlg.getByRole('button', { name: app.t('agents-start'), exact: true }).click();
    await expect(dlg).toBeHidden();
    await tab('agents-tab-runs').click();
    const queued = main.locator('tr').filter({ hasText: app.t('agents-run-queued') }).first();
    await queued.getByRole('button', { name: app.t('agents-open-run') }).click();
    await page.getByRole('dialog').getByRole('button', { name: app.t('agents-stop') }).click();
    await expect(main.locator('tr').filter({ hasText: app.t('agents-run-stopped') }).first()).toBeVisible();

    // The other tabs show where runs go.
    await tab('agents-tab-environments').click();
    await expect(main.locator('tr').filter({ hasText: `${slug}-env` })).toContainText(DIGEST.slice(0, 20));
    // The environment opens on its editor: its active build beside, a save makes v2.
    await main
      .locator('tr')
      .filter({ hasText: `${slug}-env` })
      .getByRole('button', { name: app.t('agents-env-open') })
      .click();
    await expect(main.getByText(app.t('agents-env-active'), { exact: true })).toBeVisible();
    const dockerfile = main.getByRole('textbox', { name: 'Dockerfile' });
    await expect(dockerfile).toHaveValue('FROM scratch');
    await dockerfile.fill('FROM scratch\nLABEL step=2');
    await main.getByRole('button', { name: app.t('agents-env-save') }).click();
    await expect(main.getByText('v2', { exact: true })).toBeVisible();
    await main.getByRole('button', { name: app.t('agents-env-back') }).click();
    await tab('agents-tab-runners').click();
    await expect(main.locator('tr').filter({ hasText: `${slug}-runner` })).toBeVisible();

    // Cleanup: the agent and its identity, then what it ran on.
    const agents = await api.list('/agents');
    for (const a of agents.filter((a: any) => a.name === agentName)) await api.delete(`/agents/${a.id}`);
    await api.delete(`/environments/${env.id}`).catch(() => {});
    await api.delete(`/image-registries/${registry.id}`).catch(() => {});
  });
});
