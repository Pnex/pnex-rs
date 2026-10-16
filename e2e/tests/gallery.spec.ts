// Gallery of an object (assisted-creation.md D231): a GLB uploaded from the
// object page becomes its 3D model; a joint is turned, the pose is saved
// with its thumbnail, applied again, then removed from the gallery (the
// media stays in the library).
import { expect, test } from '../src/fixtures.ts';
import { rigGlb } from '../src/rig_glb.ts';

test.describe('gallery', { tag: ['@ontology', '@media', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/maintenance/install', {}).catch(() =>
      api.post('/ontology/packs/maintenance/install', {}),
    );
  });

  test('a 3D model is posed and the pose kept with its thumbnail', async ({
    app,
    page,
    api,
    prefix,
    capture,
  }) => {
    const pump = await api.post('/ontology/objects', {
      type_key: 'pump',
      title: `${prefix} pump`,
      properties: { serial: 'G-1', status: 'running' },
    });
    await app.goto(`/ontology/object/${pump.id}`);
    await page.getByRole('tab', { name: app.t('onto-tab-gallery') }).click();
    await expect(page.getByText(app.t('gallery-empty'))).toBeVisible();

    await page.getByTestId('gallery-file').setInputFiles({
      name: `${prefix}-arm.glb`,
      mimeType: 'model/gltf-binary',
      buffer: rigGlb(),
    });
    await expect(page.locator('[data-role=model]')).toBeVisible();
    const bone = page.getByTestId('mesh-bone');
    await expect(bone).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('mesh-clip')).toContainText('Wave');

    await bone.selectOption('Elbow');
    // Sliders X, Y, Z, in that order.
    const z = page.getByRole('slider').nth(2);
    await z.fill('-60');
    await page.getByTestId('mesh-pose-name').fill(`${prefix} wave`);
    await page.getByRole('button', { name: app.t('mesh-pose-save') }).click();
    await expect(page.getByRole('button', { name: `${prefix} wave` })).toBeVisible();
    await expect(page.locator('[data-role=pose]')).toContainText(app.t('gallery-role-pose'));
    await capture('gallery-pose', { caption: 'A pose saved on an object, with its thumbnail' });

    // Rest pose, then the saved pose brings the angle back.
    await page.getByRole('button', { name: app.t('mesh-reset') }).click();
    await expect(z).toHaveValue('0');
    await page.getByRole('button', { name: `${prefix} wave` }).click();
    await expect(z).toHaveValue('-60');

    // Removing the pose closes its link; the thumbnail stays in the library.
    await page.locator('[data-role=pose]').getByRole('button', { name: app.t('gallery-remove') }).click();
    await expect(page.locator('[data-role=pose]')).toHaveCount(0);
    const media = await api.list<{ name: string }>(`/media?search=${encodeURIComponent(prefix)}`);
    expect(media.some((m) => m.name.endsWith(`${prefix} wave`))).toBe(true);
  });
});
