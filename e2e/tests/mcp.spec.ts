// MCP server (D245) through the TLS edge: handshake, tool listing, a write
// and a read run with the suite's own token and org.
import { expect, test } from '../src/fixtures.ts';

test.describe('mcp', { tag: ['@mcp'] }, () => {
  test('serves the API as tools with the caller rights', async ({ api, prefix }) => {
    let id = 0;
    const rpc = (method: string, params: unknown = {}) =>
      api.post('/mcp', { jsonrpc: '2.0', id: ++id, method, params });
    const call = async (name: string, args: unknown) => {
      const { result } = await rpc('tools/call', { name, arguments: args });
      return result;
    };

    const init = await rpc('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'e2e', version: '0' },
    });
    expect(init.result.protocolVersion).toBe('2025-06-18');

    const { result } = await rpc('tools/list');
    const names: string[] = result.tools.map((t: { name: string }) => t.name);
    expect(names).toContain('functions_create');
    expect(names).not.toContain('flows_deploy');
    expect(names).not.toContain('devices_commands');

    const name = `${prefix} mcp`;
    const created = await call('functions_create', {
      body: {
        name,
        language: 'js',
        code: '// @input value number "Value"\n// @output result number "Result"\nfunction handle(inputs, msg) {\n  return { result: inputs.value };\n}',
      },
    });
    expect(created.isError, created.content[0].text).toBe(false);

    const listed = await call('functions_list', { limit: 100 });
    expect(listed.isError).toBe(false);
    expect(listed.content[0].text).toContain(name);

    // A refused call comes back as a tool error carrying the ErrorDetail code.
    const refused = await call('functions_get', { id: 2147483647 });
    expect(refused.isError).toBe(true);
    expect(refused.content[0].text).toMatch(/^HTTP 404/);
  });
});

// What an MCP client does on its own (`claude mcp add --client-id pnex
// --callback-port 33418`): discovery from the 401, browser sign-in with
// PKCE, code exchange, refresh window — one sign-in, no pasted token.
test.describe('mcp oauth', { tag: ['@mcp'] }, () => {
  test.use({ authenticated: false });

  test('a client signs in once and refreshes by itself', async ({ page, request }) => {
    const { BASE_URL, EMAIL, PASSWORD } = await import('../src/env.ts');
    const { createHash, randomBytes } = await import('node:crypto');
    const ping = (token: string) =>
      request.post(`${BASE_URL}/api/v1/mcp`, {
        headers: { Authorization: `Bearer ${token}` },
        data: { jsonrpc: '2.0', id: 1, method: 'ping' },
      });

    const anonymous = await request.post(`${BASE_URL}/api/v1/mcp`, { data: { jsonrpc: '2.0', id: 1, method: 'ping' } });
    expect(anonymous.status()).toBe(401);
    const metadataUrl = anonymous.headers()['www-authenticate'].match(/resource_metadata="([^"]+)"/)![1];
    const metadata = await (await request.get(metadataUrl)).json();
    expect(metadata.resource).toBe(`${BASE_URL}/api/v1/mcp`);
    const issuer: string = metadata.authorization_servers[0];
    // A real client probes RFC 8414 path insertion first: those must 404,
    // not answer the SPA's HTML (200 non-JSON aborts the client).
    const issuerUrl = new URL(issuer);
    const issuerPath = issuerUrl.pathname.replace(/\/$/, '');
    for (const doc of ['oauth-authorization-server', 'openid-configuration']) {
      const probe = await request.get(`${issuerUrl.origin}/.well-known/${doc}${issuerPath}`);
      expect(probe.status() === 404 || (probe.ok() && probe.headers()['content-type']?.includes('json')), probe.url()).toBe(true);
    }
    const oidc = await (await request.get(`${issuer}.well-known/openid-configuration`)).json();

    const verifier = randomBytes(32).toString('base64url');
    const redirect = 'http://localhost:33418/callback';
    const authorize = new URL(oidc.authorization_endpoint);
    for (const [k, v] of Object.entries({
      client_id: 'pnex',
      response_type: 'code',
      redirect_uri: redirect,
      scope: metadata.scopes_supported.join(' '),
      state: 'e2e',
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256',
      resource: metadata.resource,
    }))
      authorize.searchParams.set(k, v);

    // The client's loopback listener: catch the code.
    let code = '';
    await page.route(`${redirect}**`, (route) => {
      code = new URL(route.request().url()).searchParams.get('code') ?? '';
      return route.fulfill({ body: 'ok' });
    });
    await page.goto(authorize.toString());
    await page.waitForLoadState('networkidle');
    const email = page.locator('input[type=email]:visible, input[name=email]:visible').first();
    await email.fill(EMAIL);
    await email.press('Enter');
    const password = page.locator('input[type=password]:visible').first();
    await password.fill(PASSWORD);
    await password.press('Enter');
    await expect.poll(() => code, { timeout: 30_000 }).not.toBe('');

    const exchange = await request.post(oidc.token_endpoint, {
      form: { grant_type: 'authorization_code', client_id: 'pnex', code, redirect_uri: redirect, code_verifier: verifier },
    });
    expect(exchange.ok(), await exchange.text()).toBe(true);
    const tokens = await exchange.json();
    expect((await ping(tokens.access_token)).status()).toBe(200);

    // The client refreshes by itself: the refresh token opens in the last
    // minute of the access token and outlives it (sliding, rotated on use).
    const claims = (t: string) => JSON.parse(Buffer.from(t.split('.')[1], 'base64url').toString());
    const access = claims(tokens.access_token);
    const refresh = claims(tokens.refresh_token);
    expect(refresh.nbf).toBeLessThanOrEqual(access.exp);
    expect(refresh.exp).toBeGreaterThan(access.exp + 86_400);
  });
});
