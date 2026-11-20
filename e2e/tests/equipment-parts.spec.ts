// Parts of an installed equipment (parts-docs.md PD1, Q-PD4): the parts
// list of its model kept to its serial, build date and variant; a part
// goes to its open work order, whose parts export as CSV. The scanner page
// says what to do where the browser reads no QR code.
import { expect, test } from '../src/fixtures.ts';

test.describe('equipment parts', { tag: ['@maintenance', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() => api.post('/ontology/packs/maintenance/install', {}));
  });

  test('a pump lists its parts, adds one to its work order and exports it', async ({ app, page, api, prefix }) => {
    const n = String(Date.now() % 1_000_000);
    const model = await api.post('/ontology/objects', { type_key: 'equipment_model', title: `${prefix} model`, properties: { code: `M-${n}` } });
    const part = async (no: string, title: string) =>
      api.post('/ontology/objects', { type_key: 'spare_part', title: `${prefix} ${title}`, properties: { part_number: `${no}-${n}` } });
    const lines = [
      { p: await part('A', 'fan motor'), attrs: { position: '1', quantity: 2 } },
      { p: await part('B', 'old fan'), attrs: { position: '2', quantity: 1, serial_to: 'S4999' } },
      { p: await part('C', 'gasket 115V'), attrs: { position: '3', quantity: 1, variant: '115V' } },
    ];
    for (const l of lines) {
      await api.post('/ontology/links', { link_type: 'bom_line', source_id: model.id, target_id: l.p.id, attributes: l.attrs });
    }
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump`,
      properties: { serial: 'S5200', variant: '230V', manufactured_on: '2024-03-01' },
    });
    await api.post('/ontology/links', { link_type: 'instance_of', source_id: pump.id, target_id: model.id });

    await app.goto(`/ontology/object/${pump.id}`);
    const section = page.getByTestId('equipment-parts');
    await expect(section.getByTestId('equipment-part')).toHaveCount(1);
    await expect(section).toContainText(`${prefix} fan motor`);
    await section.getByTestId('equipment-parts-all').click();
    await expect(section.getByTestId('equipment-part')).toHaveCount(3);
    await section.getByTestId('equipment-parts-all').click();
    await section.getByTestId('equipment-part-add').first().click();
    await expect(section).toContainText(app.t('eq-parts-added'));

    // The work order made for the pump holds the part, and exports it.
    const links = await api.get(`/ontology/objects/${pump.id}/links`);
    const wo = links.results.find((l: { link_type: string }) => l.link_type === 'work_order_on');
    expect(wo).toBeTruthy();
    await app.goto(`/ontology/object/${wo.source.id}`);
    const download = page.waitForEvent('download');
    await page.getByTestId('basket-export').click();
    const file = await download;
    const path = await file.path();
    const csv = (await import('node:fs')).readFileSync(path!, 'utf8');
    expect(csv).toContain(`A-${n},${prefix} fan motor,2`);
  });

  test('the scanner says what to do without a QR reader', async ({ app, page }) => {
    await app.goto('/scan');
    await expect(page.getByRole('heading', { name: app.t('scan-title') })).toBeVisible();
    const status = page.getByTestId('scan-status');
    await expect(status).toBeVisible();
    const text = await status.textContent();
    expect([app.t('scan-unsupported'), app.t('scan-denied')]).toContain(text?.trim());
  });
});
