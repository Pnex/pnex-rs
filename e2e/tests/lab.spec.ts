// Vision Lab (media-vision-studio.md D198–D200): a project, two photos of
// the media library added together (one source), a box drawn with the
// mouse, a negative example, a frozen dataset version and its COCO zip.
import { expect, test } from '../src/fixtures.ts';
import { makePng } from '../src/fixtures-files.ts';

test.describe('vision lab', { tag: ['@lab', '@i18n'] }, () => {
  test('annotate pictures and freeze a dataset version', async ({ app, api, page, browser, prefix, capture }) => {
    const name = `${prefix} valves`;
    for (const tag of ['a', 'b']) {
      await api.upload(
        `/media?filename=${tag}.png&name=${encodeURIComponent(`${prefix} ${tag}`)}&kind=photo`,
        await makePng(browser, `valve ${tag}`),
      );
    }

    await app.goto('/lab');
    await page.getByRole('main').getByRole('button', { name: app.t('lab-new') }).click();
    await page.getByRole('textbox', { name: app.t('lab-form-name'), exact: true }).fill(name);
    await page.getByRole('textbox', { name: app.t('lab-form-classes'), exact: true }).fill('red_valve\nhelmet');
    await page.getByRole('button', { name: app.t('common-save') }).click();
    const row = page.locator('main tr').filter({ hasText: name });
    await row.getByRole('link', { name: app.t('lab-open') }).click();

    // Both photos in one import.
    await page.getByRole('button', { name: app.t('lab-add-images') }).click();
    await page.getByRole('dialog').getByRole('button', { name: new RegExp(app.t('lab-add-photos')) }).click();
    await page.getByLabel(`${prefix} a`, { exact: true }).first().check();
    await page.getByLabel(`${prefix} b`, { exact: true }).first().check();
    await page.getByRole('dialog').getByRole('button', { name: /2/ }).click();
    await expect(page.getByRole('button', { name: new RegExp(`${prefix} b`) })).toBeVisible();

    // A box dragged on the first picture.
    const layer = page.getByRole('main').locator('.cursor-crosshair').first();
    await expect(page.getByRole('main').locator('img[alt]').first()).toBeVisible();
    const box = (await layer.boundingBox())!;
    await page.mouse.move(box.x + box.width * 0.2, box.y + box.height * 0.2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.7, { steps: 8 });
    await page.mouse.up();
    await expect(page.getByRole('main').getByText('red_valve', { exact: true }).last()).toBeVisible();
    await capture('lab-annotate', { caption: 'Vision Lab: a box drawn around the object' });

    // Classes added from the "+" bubble, by its button (touch) then by
    // Enter: the bubble stays open across the project reload.
    await page.getByTestId('lab-class-add').click();
    const menu = page.getByTestId('lab-class-menu');
    const field = menu.getByRole('textbox', { name: app.t('lab-class-new') });
    await field.fill('glove');
    await page.getByTestId('lab-class-submit').click();
    await expect(menu.getByRole('button', { name: /glove/ })).toBeVisible();
    await field.fill('boot');
    await field.press('Enter');
    await expect(menu.getByRole('button', { name: /boot/ })).toBeVisible();
    await expect(menu).toBeVisible();
    await page.getByTestId('lab-class-add').click();
    await expect(menu).toBeHidden();

    // The second picture marked as a negative example.
    await page.getByTestId('lab-next').click();
    await page.getByRole('button', { name: app.t('lab-negative') }).click();

    const projects: { id: string; name: string }[] = await api.list('/lab/projects');
    const id = projects.find((p) => p.name === name)!.id;
    await expect.poll(async () => (await api.get(`/lab/projects/${id}`)).annotated_count).toBe(2);

    // Training step: the versions sit under their own disclosure.
    await page.getByTestId('lab-train-open').click();
    await page.getByText(app.t('lab-versions-more'), { exact: true }).click();
    await page.getByRole('button', { name: app.t('lab-freeze') }).click();
    await expect(page.locator('details[open]').getByText(app.tr('lab-version-label')).first()).toBeVisible();
    const download = page.waitForEvent('download');
    await page.getByRole('button', { name: app.t('lab-export') }).first().click();
    expect((await download).suggestedFilename()).toMatch(/\.zip$/);
    await capture('lab-version', { caption: 'Vision Lab: a frozen dataset version, downloaded in COCO format' });

    // A source added by mistake leaves the project, its files stay in Media.
    await page.getByRole('dialog').getByRole('button', { name: app.t('common-close') }).click();
    await page.getByTestId('lab-step').filter({ hasText: app.t('lab-step-images') }).click();
    await page.getByTestId('lab-source-remove').click();
    await page.getByRole('button', { name: app.t('lab-source-remove'), exact: true }).last().click();
    await expect.poll(async () => (await api.list(`/lab/projects/${id}/images`)).length).toBe(0);
    await expect(page.getByText(app.t('lab-no-image'))).toBeVisible();
  });
});
