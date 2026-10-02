// Session bootstrap without the Rauthy UI: password grant, then the tokens are
// injected into localStorage before the app boots (session::boot reads them).
import type { APIRequestContext } from '@playwright/test';
import { CLIENT_ID, EMAIL, ISSUER, PASSWORD } from './env.ts';

export interface Tokens {
  access: string;
  refresh: string;
  id?: string;
}

export async function passwordGrant(
  request: APIRequestContext,
  email = EMAIL,
  password = PASSWORD,
): Promise<Tokens> {
  const res = await request.post(`${ISSUER}oidc/token`, {
    // Rauthy rejects token requests without a User-Agent.
    headers: { 'User-Agent': 'pnex-e2e' },
    form: { grant_type: 'password', client_id: CLIENT_ID, username: email, password },
  });
  if (!res.ok()) throw new Error(`password grant failed: ${res.status()} ${await res.text()}`);
  const json = await res.json();
  return { access: json.access_token, refresh: json.refresh_token, id: json.id_token };
}

/** localStorage keys read by the front (crates/pnex-frontend/src/storage.rs). */
export function storageEntries(tokens: Tokens, orgId: number, locale: string): Record<string, string> {
  const entries: Record<string, string> = {
    'pnex.access_token': tokens.access,
    'pnex.refresh_token': tokens.refresh,
    'pnex.org': String(orgId),
    'pnex.locale': locale,
  };
  if (tokens.id) entries['pnex.id_token'] = tokens.id;
  return entries;
}
