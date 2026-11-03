// Every pack, on the real flow runtime: the API-level tests install packs
// with the engine off, so their flows and Starlark functions only run here.
// Installed in a throwaway org, each template flow deployed, then stopped.
import { expect, test } from '../src/fixtures.ts';
import { ApiError } from '../src/api.ts';

const PACKS = [
  'companies',
  'companies_fr',
  'crm',
  'investigation',
  'investment_thesis',
  'maintenance',
  'markets',
  'media_coverage',
  'prospecting_b2b',
  'prospecting_b2b_fr',
  'quali_watch',
  'web_presence',
];

test.describe('packs', { tag: '@packs' }, () => {
  for (const pack of PACKS) {
    test(`${pack}: its flows deploy on the real runtime`, async ({ api }) => {
      test.setTimeout(240_000);
      const org = await api.post<{ id: number }>('/orgs', { name: `e2e-pack-${pack}-${Date.now().toString(36)}` });
      const scoped = api.forOrg(org.id);
      try {
        await scoped.post(`/ontology/packs/${pack}/install`, {});
        const refused: string[] = [];
        for (const flow of await scoped.list<{ id: number; name: string }>('/flows')) {
          try {
            await scoped.post(`/flows/${flow.id}/deploy`, {});
            await scoped.post(`/flows/${flow.id}/stop`, {});
          } catch (e) {
            if (!(e instanceof ApiError)) throw e;
            // A mail flow waits for the mailbox its deployer picks: that refusal is the design.
            if (e.status === 400 && e.body.includes('mail-no-mailbox')) continue;
            refused.push(`${flow.name}: ${e.status} ${e.body.slice(0, 300)}`);
          }
        }
        expect(refused).toEqual([]);
      } finally {
        await api.delete(`/orgs/${org.id}`).catch(() => {});
      }
    });
  }
});
