// Connectors by kind (connectors.md D791–D797): a webhook created from the screen
// leaves with its address and a secret shown once, and a call signed with it is
// kept; a REST connector keeps its authentication; MySQL always goes through TLS.
import { createHmac } from 'node:crypto';
import { expect, test } from '../src/fixtures.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

test.describe('connectors', { tag: '@pipelines' }, () => {
  test('a webhook gives its address and secret once', async ({ app, page, api, prefix }) => {
    const name = `${prefix} hook`;
    const main = page.getByRole('main');
    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-tab-connectors') }).click();
    await main.getByRole('button', { name: app.t('pipe-connector-add') }).click();
    const form = dialog(page, app.t('pipe-connector-add'));
    await form.getByRole('radio', { name: new RegExp(app.t('pipe-connector-kind-webhook')) }).check();
    // A webhook is called: no host to give.
    await expect(form.getByLabel(app.t('pipe-connector-host'), { exact: true })).toHaveCount(0);
    await form.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await form.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();

    const shown = dialog(page, new RegExp(escape(name)));
    await expect(shown).toBeVisible();
    const url = await shown.getByLabel(app.t('pipe-connector-url')).inputValue();
    const secret = await shown.getByLabel(app.t('pipe-connector-secret'), { exact: true }).inputValue();
    expect(url).toMatch(/\/api\/v1\/hooks\/[0-9a-f-]{36}$/);
    expect(secret).toMatch(/^[0-9a-f]{64}$/);
    await shown.getByRole('button', { name: app.t('pipe-connector-done') }).click();
    await expect(shown).toBeHidden();

    // The address takes a call signed with the secret, refuses another.
    const body = JSON.stringify({ code: 'A1', name: 'Acme' });
    const sign = (key: string) => `sha256=${createHmac('sha256', key).update(body).digest('hex')}`;
    const send = (sig: string) =>
      page.request.post(url, { headers: { 'content-type': 'application/json', 'x-signature': sig }, data: body });
    expect((await send(sign(secret))).status()).toBe(202);
    expect((await send(sign('wrong'))).status()).toBe(401);

    // Never shown again, never in a read.
    const row = main.locator('tr').filter({ hasText: name });
    await expect(row).toContainText(app.t('pipe-connector-receives'));
    await expect(main).not.toContainText(secret);
    const saved = (await api.list<any>('/connectors')).find((c) => c.name === name);
    expect(JSON.stringify(saved)).not.toContain(secret);
    await row.getByRole('button', { name: app.t('pipe-connector-test') }).click();
    await expect(page.getByText(app.t('pipe-connector-test-ok')).first()).toBeVisible();

    // A new secret is asked for, then the old one stops.
    await row.getByRole('button', { name: app.t('pipe-connector-generate') }).click();
    await confirmDialog(page, app.t('pipe-connector-generate-title'))
      .getByRole('button', { name: app.t('pipe-connector-generate') })
      .click();
    const again = dialog(page, new RegExp(escape(name)));
    const fresh = await again.getByLabel(app.t('pipe-connector-secret'), { exact: true }).inputValue();
    expect(fresh).not.toBe(secret);
    await again.getByRole('button', { name: app.t('pipe-connector-done') }).click();
    expect((await send(sign(secret))).status()).toBe(401);

    await row.getByRole('button', { name: app.t('pipe-delete') }).click();
    await confirmDialog(page, app.t('pipe-connector-delete-title')).getByRole('button', { name: app.t('pipe-delete') }).click();
    await expect(row).toHaveCount(0);
  });

  test('a REST connector keeps its authentication, MySQL keeps TLS', async ({ app, page, api, prefix }) => {
    const name = `${prefix} api`;
    const key = `key-${prefix}`;
    const main = page.getByRole('main');
    await app.goto('/pipelines');
    await main.getByRole('button', { name: app.t('pipe-tab-connectors') }).click();
    await main.getByRole('button', { name: app.t('pipe-connector-add') }).click();
    const form = dialog(page, app.t('pipe-connector-add'));
    await form.getByRole('radio', { name: new RegExp(app.t('pipe-connector-kind-rest')) }).check();
    await expect(form.getByLabel(app.t('pipe-connector-port'), { exact: true })).toHaveValue('443');
    await form.getByLabel(app.t('pipe-col-name'), { exact: true }).fill(name);
    await form.getByLabel(app.t('pipe-connector-host'), { exact: true }).fill('api.example.invalid');
    await form.getByLabel(app.t('pipe-connector-base-path'), { exact: true }).fill('/v1');
    await form.getByLabel(app.t('pipe-connector-auth')).selectOption({ label: app.t('pipe-connector-auth-header') });
    await form.getByLabel(app.t('pipe-connector-header-name'), { exact: true }).fill('X-Api-Key');
    await form.getByRole('textbox', { name: app.t('pipe-connector-secret-key') }).fill(key);
    await form.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(form).toBeHidden();

    const row = main.locator('tr').filter({ hasText: name });
    await expect(row).toContainText('https://api.example.invalid:443/v1');
    await expect(main).not.toContainText(key);
    const saved = (await api.list<any>('/connectors')).find((c) => c.name === name);
    expect(saved.rest).toMatchObject({ base_path: '/v1', auth: 'header', header_name: 'X-Api-Key' });
    expect(JSON.stringify(saved)).not.toContain(key);

    // Reopened, the screen shows the saved authentication, and the kind is fixed.
    await row.click();
    const edit = dialog(page, app.t('pipe-connector-edit'));
    await expect(edit.getByText(app.t('pipe-connector-kind-locked'))).toBeVisible();
    await expect(edit.getByLabel(app.t('pipe-connector-auth'))).toHaveValue('2');
    await expect(edit.getByLabel(app.t('pipe-connector-header-name'), { exact: true })).toHaveValue('X-Api-Key');
    await edit.getByRole('button', { name: app.t('common-cancel') }).click();
    await api.delete(`/connectors/${saved.id}`);

    // MySQL: its port, and no way to turn TLS off.
    await main.getByRole('button', { name: app.t('pipe-connector-add') }).click();
    await form.getByRole('radio', { name: new RegExp(app.t('pipe-connector-kind-mysql')) }).check();
    await expect(form.getByLabel(app.t('pipe-connector-port'), { exact: true })).toHaveValue('3306');
    await expect(form.getByText(app.t('pipe-connector-tls-required'))).toBeVisible();
    await expect(form.getByRole('checkbox', { name: app.t('pipe-connector-tls') })).toHaveCount(0);
  });
});
