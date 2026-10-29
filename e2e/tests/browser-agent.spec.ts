// Browser agent (extension-collector.md L0–L1): the extension zip served by
// the server is loaded in a real Chromium, paired with the test org, then
// runs two missions on a local page: a value read into a series, a page
// kept as a document. A page on an origin the agent was not allowed stays
// blocked with `ext-permission-missing` (never a silent bypass).
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium } from '@playwright/test';
import { expect, test } from '../src/fixtures.ts';
import { BASE_URL, ORG_NAME } from '../src/env.ts';

test.describe('browser agent', { tag: ['@extension'] }, () => {
  test('extension: paired agent reads a value and keeps a page', async ({ api, tokens, runState, prefix }) => {
    test.setTimeout(180_000);
    const dir = mkdtempSync(path.join(tmpdir(), 'pnex-ext-'));
    writeFileSync(path.join(dir, 'ext.zip'), await api.download('/extension/download'));
    execFileSync('unzip', ['-q', path.join(dir, 'ext.zip'), '-d', path.join(dir, 'ext')]);
    // Headless Chromium denies every permission prompt: the server's origin
    // and localhost (the test page) are granted up front, as the operator's
    // "Allow" click would.
    const manifest = path.join(dir, 'ext', 'manifest.json');
    const granted = ['http://localhost/*', `${new URL(BASE_URL).origin}/*`];
    writeFileSync(
      manifest,
      JSON.stringify({ ...JSON.parse(readFileSync(manifest, 'utf8')), host_permissions: granted }),
    );

    // The page the missions read, on localhost (the server's host, allowed at
    // pairing) and on 127.0.0.1 (never allowed).
    const site = createServer((_, res) => {
      res.setHeader('Content-Type', 'text/html');
      res.end('<!doctype html><title>Prices</title><p>Price: <span class="price">42.5</span></p>');
    });
    await new Promise<void>((ok) => site.listen(0, ok));
    const port = (site.address() as AddressInfo).port;

    const ext = path.join(dir, 'ext');
    const ctx = await chromium.launchPersistentContext(path.join(dir, 'profile'), {
      // The headless shell loads no extension: full Chromium, new headless.
      channel: 'chromium',
      headless: true,
      // Behind the local TLS edge the certificate is self-signed.
      ignoreHTTPSErrors: true,
      args: [`--disable-extensions-except=${ext}`, `--load-extension=${ext}`, '--ignore-certificate-errors'],
    });
    const created: string[] = [];
    const before = new Set((await api.get('/extension/agents?limit=100')).results.map((a: { id: number }) => a.id));
    try {
      const sw = ctx.serviceWorkers()[0] ?? (await ctx.waitForEvent('serviceworker'));
      const options = await ctx.newPage();
      await options.goto(`chrome-extension://${new URL(sw.url()).host}/options.html`);
      // Prefilled from the zip's server.json (the public origin, may differ
      // from BASE_URL behind an edge).
      await expect(options.locator('#server')).not.toHaveValue('');
      const name = await options.evaluate(
        async ([server, token, org, orgName]) => {
          // Module of the extension page, unknown to the test's type checker.
          const pkg = './pkg/pnex_extension.js';
          const m = await import(pkg);
          await m.default();
          const name = await m.pair(server, token, org, orgName);
          (globalThis as any).chrome.runtime.sendMessage({ t: 'wake' });
          return name;
        },
        [BASE_URL, tokens.access, runState.orgId, ORG_NAME] as const,
      );
      expect(name).toBeTruthy();

      const mission = async (label: string, spec: unknown) => {
        const m = await api.post('/extension/missions', { name: `${prefix} ${label}`, spec, interval_secs: 60 });
        created.push(m.id);
        return m.id as string;
      };
      const page = `http://localhost:${port}/prices.html`;
      const value = await mission('value', {
        kind: 'collect_value',
        params: { url: page, selector: '.price', metric: `price_${port}` },
      });
      const kept = await mission('page', { kind: 'collect_page', params: { url: `${page}?kept` } });
      const denied = await mission('denied', {
        kind: 'collect_value',
        params: { url: `http://127.0.0.1:${port}/prices.html`, selector: '.price', metric: `denied_${port}` },
      });

      const state = async (id: string) => {
        const m = await api.get(`/extension/missions/${id}`);
        return `${m.state}:${m.state_code ?? ''}`;
      };
      await expect.poll(() => state(value), { timeout: 120_000 }).toBe('running:');
      await expect.poll(() => state(kept), { timeout: 120_000 }).toBe('running:');
      await expect.poll(() => state(denied), { timeout: 120_000 }).toBe('blocked:ext-permission-missing');
      // The kept page may land after the state flip; filter by name so other docs don't page it out.
      const search = encodeURIComponent(`${prefix} page`);
      await expect.poll(async () => (await api.get(`/media?kind=document&search=${search}&limit=100`)).results
        .some((d: { name: string }) => d.name === `${prefix} page`), { timeout: 30_000 }).toBe(true);
    } finally {
      for (const id of created) await api.delete(`/extension/missions/${id}`).catch(() => {});
      for (const a of (await api.get('/extension/agents?limit=100')).results) {
        if (!before.has(a.id)) await api.delete(`/devices/${a.id}`).catch(() => {});
      }
      await ctx.close();
      site.close();
    }
  });
});
