// Test organization: found or created by name, optionally recreated.
import type { Api } from './api.ts';

interface OrgRow {
  id: number;
  name: string;
  role: string;
}

export async function ensureOrg(api: Api, name: string, fresh: boolean): Promise<number> {
  const existing = (await api.list<OrgRow>('/orgs')).find((o) => o.name === name);
  if (existing && fresh) {
    await api.delete(`/orgs/${existing.id}`);
  } else if (existing) {
    return existing.id;
  }
  const created = await api.post<{ id: number }>('/orgs', { name });
  return created.id;
}

/**
 * Deletes organizations created by tests (name starting with `prefix`,
 * "e2e-" by convention) — never any other organization.
 */
export async function sweepTestOrgs(api: Api, prefix = 'e2e-'): Promise<void> {
  for (const o of await api.list<OrgRow>('/orgs')) {
    if (o.name.startsWith(prefix) && o.role === 'owner') await api.delete(`/orgs/${o.id}`).catch(() => {});
  }
}
