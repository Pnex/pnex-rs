// Spare parts (parts-docs.md PD0, D961): a reference typed any way finds
// its part, and a superseded part shows the one replacing it. Worst case
// (break-ui): very long titles, many references, a full page.
import { expect, test } from '../src/fixtures.ts';

test.describe('spare parts', { tag: ['@maintenance', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() => api.post('/ontology/packs/maintenance/install', {}));
  });

  test('a reference typed with separators finds the part and its replacement', async ({ app, page, api, prefix }) => {
    const n = String(Date.now() % 1_000_000);
    const part = (no: string, title: string) =>
      api.post('/ontology/objects', { type_key: 'spare_part', title: `${prefix} ${title}`, properties: { part_number: `${no}-${n}` } });
    const old = await part('OLD', 'fan motor');
    const fresh = await part('NEW', 'fan motor v2');
    const ref = await api.post('/ontology/objects', {
      type_key: 'part_reference',
      title: `zx-${n}.a`,
      properties: { reference: `zx-${n}.a`, nature: 'origin' },
    });
    await api.post('/ontology/links', { link_type: 'refers_to', source_id: ref.id, target_id: old.id });
    await api.post('/ontology/links', { link_type: 'supersedes', source_id: fresh.id, target_id: old.id });

    await app.goto('/parts');
    await expect(page.getByRole('heading', { name: app.t('parts-title') })).toBeVisible();
    await expect(page.getByTestId('parts-searched-as')).toHaveText(app.t('parts-search-help'));
    await page.getByTestId('parts-search').fill(`ZX ${n}`);
    await expect(page.getByTestId('parts-searched-as')).toContainText(`ZX${n}`);
    const row = page.getByRole('row').filter({ hasText: `${prefix} fan motor` }).first();
    await expect(row).toContainText(`zx-${n}.a`);
    await expect(row).toContainText(`${prefix} fan motor v2`);
    await row.click();
    await expect(page).toHaveURL(new RegExp(`/ontology/object/${old.id}`));
  });

  test('worst case: long titles, many references, a full page', async ({ app, page, capture }) => {
    const long = `Compressor fan motor ${'with an unusually long designation '.repeat(6)}end`;
    const obj = (i: number, type_key: string, title: string) => ({
      id: `00000000-0000-4000-8000-${String(i).padStart(12, '0')}`,
      type_key,
      title,
      native_id: String(i),
      system: false,
      properties: { part_number: `PN-${'9'.repeat(40)}-${i}` },
      version: 1,
      valid_from: '2026-01-01T00:00:00Z',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    });
    await page.route('**/api/v1/ontology/query', (r) =>
      r.fulfill({ json: { count: 120, rows: Array.from({ length: 20 }, (_, i) => ({ object: obj(i, 'spare_part', i === 0 ? long : `Part ${i}`), latest: {} })) } }),
    );
    await page.route('**/api/v1/ontology/objects/*/links*', (r) => {
      const id = r.request().url().split('/objects/')[1].split('/')[0];
      const refs = Array.from({ length: 12 }, (_, k) => ({
        id: k + 1,
        link_type: 'refers_to',
        source: { id: `ref-${k}`, title: `REF-${'X'.repeat(30)}-${k}`, type_key: 'part_reference', native_id: `ref-${k}` },
        target: { id, title: 'Part', type_key: 'spare_part', native_id: id },
        attributes: {},
        valid_from: '2026-01-01T00:00:00Z',
      }));
      const newer = id.endsWith('1')
        ? [{ id: 99, link_type: 'supersedes', source: { id: 'newer', title: long, type_key: 'spare_part', native_id: 'newer' }, target: { id, title: 'Part', type_key: 'spare_part', native_id: id }, attributes: {}, valid_from: '2026-01-01T00:00:00Z' }]
        : [];
      return r.fulfill({ json: { count: refs.length + newer.length, next: null, previous: null, results: [...refs, ...newer] } });
    });
    await app.goto('/parts');
    await page.getByTestId('parts-search').fill('ref');
    const row = page.getByRole('row').filter({ hasText: 'Part 3' });
    await expect(row).toBeVisible();
    // Three references shown, the others counted.
    await expect(row).toContainText(`REF-${'X'.repeat(30)}-2`);
    await expect(row).not.toContainText(`REF-${'X'.repeat(30)}-3`);
    // The long names wrap inside their row, never past the page.
    const width = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(width).toBeLessThanOrEqual(1);
    await capture('parts-search', { caption: 'Spare parts found by any spelling of a reference' });
  });
});
