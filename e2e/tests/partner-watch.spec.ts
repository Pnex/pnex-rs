// Partner network watch (network-check.md D426–D431, D415–D420), on the
// quali_watch pack. Each test works in an organization of its own
// ("e2e-watch-…", deleted by the next global setup): the pack's unique
// keys (certification code, SIREN, watch key) never collide between runs,
// and its draft flows are there to deploy.
//
// 1. A deployed flow records check runs: the watch case opens under its
//    certification's case, only a changed result adds an event, and the
//    case's Checks panel shows the check, its result and its runs.
// 2. Analyze on the reference then the partner programme (the RS0101 ×
//    OF EXEMPLE pilot): modules, PROG findings, the cross-table workbook.
// 3. A crawl keeps the PDF a relevant page links (D421): one media, one
//    `has_document` link, the same bytes read twice stay one version.
import { readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { networkInterfaces } from 'node:os';
import path from 'node:path';
import type { Page } from '@playwright/test';
import { expect, test } from '../src/fixtures.ts';
import type { Api } from '../src/api.ts';
import { dialog } from '../src/pages/shell.ts';

const FIXTURES = path.join(import.meta.dirname, '..', 'fixtures', 'watch');

/** A fresh organization with the pack, and the UI switched to it. */
async function watchOrg(api: Api, page: Page, prefix: string): Promise<Api> {
  const name = `e2e-watch-${prefix.toLowerCase()}-${Date.now().toString(36)}`;
  const org = await api.post<{ id: number }>('/orgs', { name });
  const scoped = api.forOrg(org.id);
  await scoped.post('/ontology/packs/quali_watch/install', {});
  await page.goto('/');
  await page.evaluate((id) => localStorage.setItem('pnex.org', String(id)), org.id);
  return scoped;
}

/** A small site served from this machine, reachable by the server on the
 * LAN (the default `lan` egress policy allows private addresses). */
async function serveSite(): Promise<{ server: Server; root: string }> {
  const pdf = Buffer.from('%PDF-1.4\n1 0 obj << >> endobj\n%%EOF\n');
  const pages: Record<string, [string, string | Buffer]> = {
    '/robots.txt': ['text/plain', 'User-agent: *\n'],
    '/': ['text/html', '<html><head><title>Home</title></head><body><a href="/training/rs">RS</a></body></html>'],
    '/training/rs': [
      'text/html',
      '<html><head><title>Training</title></head><body><h1>Certification RS0101</h1>' +
        '<a href="/docs/programme.pdf">Programme</a></body></html>',
    ],
    '/docs/programme.pdf': ['application/pdf', pdf],
  };
  const server = createServer((req, res) => {
    const page = pages[(req.url ?? '/').split('?')[0]];
    res.writeHead(page ? 200 : 404, { 'content-type': page?.[0] ?? 'text/plain' });
    res.end(page?.[1] ?? '');
  });
  await new Promise<void>((done) => server.listen(0, '0.0.0.0', done));
  const ip = Object.values(networkInterfaces())
    .flat()
    .find((a) => a && a.family === 'IPv4' && !a.internal && !a.address.startsWith('172.'))?.address;
  if (!ip) throw new Error('no LAN address to serve the site from');
  return { server, root: `http://${ip}:${(server.address() as AddressInfo).port}/` };
}

async function object(api: Api, type_key: string, title: string, properties: object) {
  return api.post('/ontology/objects', { type_key, title, properties });
}

async function link(api: Api, link_type: string, source: { id: string }, target: { id: string }) {
  return api.post('/ontology/links', { link_type, source_id: source.id, target_id: target.id });
}

async function query(api: Api, type_key: string, filters: object[] = []) {
  return (await api.post('/ontology/query', { type_key, filters, limit: 200 })).rows.map((r: any) => r.object);
}

/** A deployed flow injecting `runs` once into a check_record node. */
async function deployRuns(api: Api, name: string, runs: object[]) {
  const nodes: object[] = runs.map((run, i) => ({
    id: `i${i}`,
    kind: 'inject',
    config: { once_delay_secs: 1 + i * 3, payload: run },
    outputs: [{ port: 0, targets: ['check'] }],
  }));
  nodes.push({ id: 'check', kind: 'check_record' });
  const flow = await api.post('/flows', { name, graph: { nodes } });
  await api.post(`/flows/${flow.id}/deploy`, {});
  return flow;
}

test.describe('partner network watch', { tag: ['@watch', '@i18n'] }, () => {
  test('check runs open a watch case and fill its Checks panel', async ({ app, page, api, prefix, capture }) => {
    const watch = await watchOrg(api, page, prefix);
    const cert = await object(watch, 'certification', 'RS0101', { code: 'RS0101' });
    const partner = await object(watch, 'organization', 'OF EXEMPLE', { siren: '100000009' });
    const finding = await object(watch, 'finding', 'ADM-02 · OF EXEMPLE', {
      ref: 'ADM-02:100000009',
      state: 'proposed',
    });
    const run = (result: string, extra: object = {}) => ({
      check: 'quali_watch.network',
      subject_id: partner.id,
      scope_ids: [cert.id],
      rules: [{ code: 'ADM-02', version: 1, result, ...extra }],
      case: {
        key: 'qw:RS0101:100000009',
        title: 'Watch RS0101 · OF EXEMPLE',
        parent_key: 'qw:RS0101',
        parent_title: 'Watch RS0101',
      },
    });
    // Two identical runs, then a change: two events, not three.
    await deployRuns(watch, `${prefix} checks`, [
      run('ok'),
      run('ok'),
      run('finding', { finding_id: finding.id }),
    ]);

    const byKey = () => query(watch, 'case', [{ key: 'watch_key', op: 'eq', value: 'qw:RS0101:100000009' }]);
    await expect.poll(async () => (await byKey())[0]?.properties.watch_state, { timeout: 60_000 }).toBe('finding');
    const kase = (await byKey())[0];
    const ws = await watch.get(`/cases/${kase.id}/workspace`);
    expect(ws.events).toHaveLength(2);
    expect(ws.events.every((e: any) => e.event.properties.origin === 'flow')).toBe(true);
    expect(ws.parents[0].title).toBe('Watch RS0101');
    expect(ws.checks[0]).toMatchObject({ check: 'quali_watch.network', result: 'finding', runs_since_change: 1 });

    // The workspace shows the check: its result and the rule not OK.
    await app.goto(`/cases/${kase.id}`);
    const panel = page.getByTestId('case-checks');
    await expect(panel).toContainText(app.t('cases-checks-title'));
    const row = panel.getByTestId('case-check');
    await expect(row).toContainText('quali_watch.network');
    await expect(row).toContainText(app.t('cases-checks-result-finding'));
    await expect(row).toContainText('ADM-02');
    await capture('watch-case-checks', { caption: 'A watch case: its check, the rule not OK, since when' });
  });

  test('Analyze turns two programmes into a cross-table', async ({ app, page, api, prefix, capture }) => {
    test.setTimeout(240_000);
    const watch = await watchOrg(api, page, prefix);
    const cert = await object(watch, 'certification', 'RS0101', {
      code: 'RS0101',
      official_title: "Piloter le référencement d'un site",
    });
    for (let i = 1; i <= 6; i++) {
      const skill = await object(watch, 'skill', `C${i}`, { ref: `RS0101-C${i}`, rank: i, label: `Competency ${i}` });
      await link(watch, 'skill_of', skill, cert);
    }
    const partner = await object(watch, 'organization', 'OF EXEMPLE', { siren: '100000009' });
    const programme = async (role: string) => {
      const media = await watch.upload(`/media?filename=${role}.txt`, readFileSync(path.join(FIXTURES, `${role}.txt`)));
      const asset = await watch.get(`/ontology/resolve/media_asset/${media.id}`);
      const s = await object(watch, 'syllabus', `${role} programme`, { ref: `RS0101:${role}`, role, source: asset.id });
      await link(watch, 'syllabus_for', s, cert);
      if (role === 'partner') await link(watch, 'syllabus_of', s, partner);
      return s;
    };
    const reference = await programme('reference');
    const partnerProgramme = await programme('partner');
    const flow = (await watch.list('/flows')).find((f: any) => f.name === 'Quali-Watch · cross-table');
    expect(flow, 'the pack ships the cross-table flow').toBeTruthy();
    await watch.post(`/flows/${flow.id}/deploy`, {});

    // Analyze from the programme page: the action dialog, then the run.
    const analyze = async (id: string) => {
      await app.goto(`/ontology/object/${id}`);
      await page.getByRole('button', { name: /^(Analyze|Analyser)$/ }).click();
      const dlg = dialog(page, app.tr('onto-action-title'));
      await dlg.getByRole('button', { name: app.t('onto-action-run') }).click();
      await expect(dlg).toBeHidden();
    };
    // A deploy elsewhere reloads the one flow runtime and drops the messages
    // in flight (another test's org): Analyze again, as a user would; the
    // writes are idempotent (modules by key, findings created once).
    const until = async (id: string, done: () => Promise<boolean>) => {
      for (let attempt = 0; attempt < 3; attempt++) {
        await analyze(id);
        try {
          await expect.poll(done, { timeout: 40_000 }).toBe(true);
          return;
        } catch {}
      }
      throw new Error(`Analyze of ${id} never completed`);
    };
    const modules = () => query(watch, 'module');
    await until(reference.id, async () => (await modules()).length === 6);

    await until(partnerProgramme.id, async () => (await modules()).length === 13);
    const prog = async () =>
      (await query(watch, 'finding'))
        .map((f: any) => f.properties.ref as string)
        .filter((r: string) => r.startsWith('PROG-'))
        .sort();
    await expect
      .poll(prog, { timeout: 90_000 })
      .toEqual([
        'PROG-02:100000009:RS0101:C2',
        'PROG-02:100000009:RS0101:C3',
        'PROG-02:100000009:RS0101:C4',
        'PROG-03:100000009:RS0101:5',
      ]);
    expect((await modules()).length).toBe(13);

    // The workbook is in the media library, readable as a table.
    const media = await watch.list('/media');
    const book = media.find((m: any) => m.name === 'Tableau croisé RS0101 OF EXEMPLE.xlsx');
    expect(book, 'the cross-table workbook').toBeTruthy();
    await app.goto('/media');
    await expect(page.getByRole('main')).toContainText('Tableau croisé RS0101 OF EXEMPLE.xlsx');
    await capture('cross-table-workbook', { caption: 'The cross-table written by the Analyze action' });
  });

  test('a crawl keeps the document a relevant page links', async ({ app, page, api, prefix, capture }) => {
    test.setTimeout(120_000);
    const watch = await watchOrg(api, page, prefix);
    const { server, root } = await serveSite();
    try {
      const host = new URL(root).hostname;
      const site = await object(watch, 'website', host, { domain: host, root_url: root });
      // Two passes of the same crawl: the second finds the same bytes.
      const crawl = (i: number) => ({
        id: `i${i}`,
        kind: 'inject',
        config: {
          once_delay_secs: 1 + i * 20,
          payload: { url: root, match: [{ kind: 'regex', value: 'RS\\s?0101' }], link_to: site.id },
        },
        outputs: [{ port: 0, targets: ['crawl'] }],
      });
      const graph = {
        nodes: [
          crawl(0),
          crawl(1),
          {
            id: 'crawl',
            kind: 'site_crawl',
            config: {
              max_pages: 10,
              proofs: false,
              record: {
                type: 'observed_page',
                properties: { url: 'url', title: 'heading' },
                link_type: 'page_of',
                document_link: 'has_document',
              },
            },
          },
        ],
      };
      const flow = await watch.post('/flows', { name: `${prefix} crawl`, graph });
      await watch.post(`/flows/${flow.id}/deploy`, {});

      const pages = () => query(watch, 'observed_page');
      await expect.poll(async () => (await pages()).length, { timeout: 60_000 }).toBe(1);
      const observed = (await pages())[0];
      expect(observed.properties.url).toBe(`${root}training/rs`);
      const documents = async () =>
        (await watch.list(`/ontology/objects/${observed.id}/links`)).filter(
          (l: any) => l.link_type === 'has_document' && !l.valid_to,
        );
      await expect.poll(async () => (await documents()).length, { timeout: 30_000 }).toBe(1);
      const [doc] = await documents();
      expect(doc.attributes.url).toBe(`${root}docs/programme.pdf`);

      // The media of the crawl: one asset, still one version after the
      // second pass (the deploy's second inject, 20 s later).
      const asset = async () => (await watch.list('/media')).find((m: any) => m.name === 'programme.pdf');
      await expect.poll(async () => !!(await asset()), { timeout: 30_000 }).toBe(true);
      const media = await asset();
      await page.waitForTimeout(25_000);
      expect(await watch.list(`/media/${media.id}/versions`)).toHaveLength(1);
      expect(await documents()).toHaveLength(1);

      // The page object shows the document it links.
      await app.goto(`/ontology/object/${observed.id}`);
      await page.getByRole('tab', { name: app.t('onto-tab-links') }).click();
      await expect(page.getByRole('main')).toContainText('programme.pdf');
      await capture('crawl-linked-document', { caption: 'A crawled page and the document it links' });
    } finally {
      server.close();
    }
  });
});
