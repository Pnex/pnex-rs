// Resolves the test org once per run and empties it, so every run starts
// from the same state. The org id is handed to workers through STATE_FILE.
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { request } from '@playwright/test';
import { Api, SWEPT_COLLECTIONS, sweep } from './src/api.ts';
import { passwordGrant } from './src/auth.ts';
import { BASE_URL, FRESH_ORG, ORG_NAME, STATE_FILE } from './src/env.ts';
import { ensureOrg, sweepTestOrgs } from './src/org.ts';

export default async function globalSetup(): Promise<void> {
  const ctx = await request.newContext({ ignoreHTTPSErrors: true });
  try {
    const health = await ctx.get(`${BASE_URL}/health/ready`).catch(() => undefined);
    if (!health?.ok()) throw new Error(`PNeX unreachable at ${BASE_URL} (task app:up / task dev?)`);

    const tokens = await passwordGrant(ctx);
    const api = new Api(ctx, tokens.access);
    const orgId = await ensureOrg(api, ORG_NAME, FRESH_ORG);
    if (process.env.PNEX_E2E_KEEP !== '1') {
      await sweep(api.forOrg(orgId), '');
      await sweepTestOrgs(api);
    }

    mkdirSync(path.dirname(STATE_FILE), { recursive: true });
    writeFileSync(STATE_FILE, JSON.stringify({ orgId, orgName: ORG_NAME }, null, 2));
    console.log(`e2e: ${BASE_URL}, org "${ORG_NAME}" #${orgId} (swept: ${SWEPT_COLLECTIONS.join(', ')})`);
  } finally {
    await ctx.dispose();
  }
}
