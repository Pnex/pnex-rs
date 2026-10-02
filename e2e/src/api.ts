// Thin REST client for test setup, assertions and cleanup.
//
// Tests drive features through the UI; the API only seeds preconditions,
// checks what the UI persisted, and removes what a test created.
import type { APIRequestContext, APIResponse } from '@playwright/test';
import { BASE_URL } from './env.ts';

export interface Page<T> {
  count: number;
  results: T[];
}

export class ApiError extends Error {
  constructor(
    readonly method: string,
    readonly path: string,
    readonly status: number,
    readonly body: string,
  ) {
    super(`${method} ${path} -> ${status} ${body.slice(0, 300)}`);
  }
}

export class Api {
  constructor(
    private readonly request: APIRequestContext,
    private readonly access: string,
    readonly orgId?: number,
  ) {}

  /** Same client scoped to another org (`X-Org-Id`). */
  forOrg(orgId: number): Api {
    return new Api(this.request, this.access, orgId);
  }

  private headers(): Record<string, string> {
    const h: Record<string, string> = { Authorization: `Bearer ${this.access}` };
    if (this.orgId != null) h['X-Org-Id'] = String(this.orgId);
    return h;
  }

  private async send(method: string, path: string, data?: unknown): Promise<APIResponse> {
    const res = await this.request.fetch(`${BASE_URL}/api/v1${path}`, {
      method,
      headers: this.headers(),
      data,
    });
    if (!res.ok()) throw new ApiError(method, path, res.status(), await res.text());
    return res;
  }

  async get<T = any>(path: string): Promise<T> {
    return (await this.send('GET', path)).json();
  }

  async post<T = any>(path: string, data?: unknown): Promise<T> {
    const res = await this.send('POST', path, data);
    return res.status() === 204 ? (undefined as T) : res.json();
  }

  async patch<T = any>(path: string, data?: unknown): Promise<T> {
    const res = await this.send('PATCH', path, data);
    return res.status() === 204 ? (undefined as T) : res.json();
  }

  async delete(path: string): Promise<void> {
    await this.send('DELETE', path);
  }

  /** Every item of a paginated list (`{ count, results }`). */
  async list<T = any>(path: string): Promise<T[]> {
    const sep = path.includes('?') ? '&' : '?';
    const out: T[] = [];
    for (let offset = 0; ; ) {
      const page = await this.get<Page<T>>(`${path}${sep}limit=100&offset=${offset}`);
      out.push(...page.results);
      offset += page.results.length;
      if (!page.results.length || offset >= page.count) return out;
    }
  }

  /** Items of a list whose `name` starts with `prefix`. */
  async named<T extends { name: string }>(path: string, prefix: string): Promise<T[]> {
    return (await this.list<T>(path)).filter((r) => r.name?.startsWith(prefix));
  }
}

/**
 * Collections emptied by `sweep()`. The test org belongs to the suite, so
 * the global setup sweeps it whole (`prefix = ''`); a test may sweep its own
 * prefix to stay independent of its neighbours.
 */
export const SWEPT_COLLECTIONS = ['/functions', '/flows', '/dashboards'] as const;

export async function sweep(api: Api, prefix: string): Promise<void> {
  for (const path of SWEPT_COLLECTIONS) {
    let items: { id: number | string; name: string }[];
    try {
      items = await api.named(path, prefix);
    } catch {
      continue;
    }
    for (const it of items) await api.delete(`${path}/${it.id}`).catch(() => {});
  }
}
