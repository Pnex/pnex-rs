// Studio: a two-scene virtual tour on a floor plan, saved, then deleted.
import { expect, test } from '../src/fixtures.ts';
import { makeFloorPlan, makePanorama } from '../src/fixtures-files.ts';
import { confirmDialog, dialog } from '../src/pages/shell.ts';

interface TourDetail {
  id: string;
  name: string;
  doc: { floors?: unknown[]; scenes?: { id: string; label?: string | null }[]; start_scene?: string | null };
}

test.describe('studio', { tag: '@studio' }, () => {
  test('tour with a floor plan and two scenes', async ({ app, api, page, browser, prefix, capture }) => {
    // Media preconditions (the upload UI has its own spec).
    const q = (name: string, kind: string) => `/media?filename=${kind}.png&name=${encodeURIComponent(`${prefix} ${name}`)}&kind=${kind}`;
    const plan = await api.upload<{ id: string }>(q('plan', 'floorplan'), await makeFloorPlan(browser, ['Boiler room', 'Office', 'Store']));
    const boiler = await api.upload<{ id: string }>(q('boiler room', 'panorama'), await makePanorama(browser, 'Boiler room'));
    const office = await api.upload<{ id: string }>(q('office', 'panorama'), await makePanorama(browser, 'Office'));

    const name = `${prefix} plant tour`;
    await app.goto('/studio');
    const main = page.getByRole('main');
    await main.getByRole('button', { name: app.t('studio-new') }).click();
    // Rename through the editor shell title (its tooltip says "Rename"),
    // never a position in the toolbar: buttons are added there over time.
    await main.getByTitle(app.t('eshell-rename')).first().click();
    const title = main.locator('input:not([placeholder])').first();
    await title.fill(name);
    await title.press('Enter');
    await expect(main.getByRole('button', { name })).toBeVisible();
    // The floors panel is a popover: open it after the rename (a click
    // elsewhere closes it).
    await main.getByRole('button', { name: app.t('studio-floors-title') }).click();

    // Floor plan of the ground floor.
    await main.getByRole('button', { name: /^Ground floor|^Rez/ }).first().click();
    await page.getByRole('button', { name: app.t('studio-floor-plan-pick') }).click();
    await dialog(page, /./).getByRole('button', { name: new RegExp(plan.id) }).click();

    // Two scenes, each from its panorama, labelled.
    const addScene = async (mediaId: string, label: string) => {
      await main.getByRole('button', { name: app.t('eshell-add'), exact: true }).click();
      await main.getByRole('button', { name: app.t('studio-add-scene') }).click();
      await dialog(page, /./).getByRole('button', { name: new RegExp(mediaId) }).click();
      const inspector = main.getByRole('complementary').last();
      await inspector.getByRole('textbox', { name: app.t('studio-scene-label') }).fill(label);
      return inspector;
    };
    const first = await addScene(boiler.id, 'Boiler room');
    await first.getByRole('button', { name: app.t('studio-scene-start') }).click();
    await addScene(office.id, 'Office');
    await capture('studio-tour', { caption: 'Two scenes on the floor plan' });

    await main.getByRole('button', { name: app.t('common-save') }).click();
    await expect(main.getByRole('button', { name: app.t('common-save') })).toBeDisabled();

    // The button is also disabled while saving: wait for the stored doc.
    await expect
      .poll(async () => {
        const saved = (await api.list<{ id: string; name: string }>('/tours')).find((t) => t.name === name);
        if (!saved) return [];
        const detail = await api.get<TourDetail>(`/tours/${saved.id}`);
        return (detail.doc.scenes ?? []).map((s) => s.label).sort();
      })
      .toEqual(['Boiler room', 'Office']);

    await app.goto('/studio');
    const row = page.locator('main tr').filter({ hasText: name });
    await row.getByRole('button', { name: app.t('studio-delete') }).click();
    await confirmDialog(page, app.t('studio-confirm-delete-title')).getByRole('button', { name: app.t('studio-delete') }).click();
    await expect(row).toHaveCount(0);
  });
});
