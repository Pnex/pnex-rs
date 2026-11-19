// QR tags (parts-docs.md D962–D963): an object shows its QR and prints it;
// a blank sheet is made, a blank tag scanned is linked to an object and
// then opens it. Worst case (break-ui): a full page of tags in every state.
import { expect, test } from '../src/fixtures.ts';

test.describe('qr tags', { tag: ['@maintenance', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() => api.post('/ontology/packs/maintenance/install', {}));
  });

  test('an object shows its QR, a blank tag is linked on site', async ({ app, page, api, prefix }) => {
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump`,
      properties: { serial: `SN-${Date.now() % 1_000_000}` },
    });
    await app.goto(`/ontology/object/${pump.id}`);
    await page.getByTestId('object-qr-show').click();
    await expect(page.getByTestId('object-qr-code').locator('svg')).toBeVisible();
    await page.getByRole('button', { name: app.t('qr-print'), exact: true }).click();
    await expect(page.getByTestId('qr-label')).toHaveCount(1);
    await expect(page.getByTestId('qr-label')).toContainText(`${prefix} pump`);

    // A blank sheet, then the scan of one of its labels.
    await app.goto('/qr-tags');
    await page.locator('#qr-sheet-count').fill('3');
    await page.getByTestId('qr-sheet-blank').click();
    await expect(page.getByTestId('qr-label')).toHaveCount(3);
    const url = new URL(page.url());
    const token = url.searchParams.get('tokens')!.split(',')[0];
    await app.goto(`/q/${token}`);
    await expect(page.getByTestId('qr-blank')).toHaveText(app.t('qr-blank'));
    await page.getByTestId('qr-link-search').fill(`${prefix} pump`);
    await page.getByTestId('qr-link-here').first().click();
    await expect(page).toHaveURL(new RegExp(`/ontology/object/${pump.id}`));
    // Scanned again: straight to the pump.
    await app.goto(`/q/${token}`);
    await expect(page).toHaveURL(new RegExp(`/ontology/object/${pump.id}`));
    await app.goto('/q/aaaaaaaaaaaaaaaaaaaaaaaaaa');
    await expect(page.getByTestId('qr-unknown')).toHaveText(app.t('qr-unknown'));
  });

  test('worst case: a full page of tags in every state', async ({ app, page, capture }) => {
    const states = ['blank', 'linked', 'revoked'];
    const long = `Compressor ${'with an extraordinarily long name '.repeat(6)}end`;
    await page.route('**/api/v1/qr-tags?*', (r) =>
      r.fulfill({
        json: {
          count: 260,
          next: null,
          previous: null,
          results: Array.from({ length: 20 }, (_, i) => ({
            token: `${'abcdefghijklmnopqrstuvwxyz'.slice(i % 20, i % 20 + 6)}${'a'.repeat(20)}`,
            state: states[i % 3],
            ...(i % 3 === 1 ? { object_id: '00000000-0000-4000-8000-000000000001', object_title: i === 1 ? long : `Pump ${i}`, object_type: 'pump' } : {}),
            created_at: '2026-11-19T22:00:00Z',
          })),
        },
      }),
    );
    await app.goto('/qr-tags');
    for (const s of states) {
      await expect(page.getByText(app.t(`qr-state-${s}`)).first()).toBeVisible();
    }
    const width = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(width).toBeLessThanOrEqual(1);
    await capture('qr-tags', { caption: 'QR tags of the organization in every state' });
  });
});
