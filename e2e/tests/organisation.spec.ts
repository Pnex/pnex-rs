// Organisation of resources: one label mechanism everywhere (D42), the
// Sites view of the site tree, the Location breadcrumb of a detail page,
// and the board change of a registered device (O39).
import { expect, test } from '../src/fixtures.ts';
import { DevicesPage } from '../src/pages/devices.ts';

/** Device ids are 16 characters max: short, unique per run. */
function deviceId(tag: string): string {
  return `${tag}${Date.now().toString(36).slice(-6)}`;
}

async function createDevice(api: any, id: string): Promise<number> {
  const created = await api.post('/devices', { device_id: id, predefined_device_name: 'generic_esp8266' });
  return created.id as number;
}

test.describe('organisation', { tag: '@sites' }, () => {
  test('device labels: chips editor, list filter', async ({ app, api, capture }) => {
    const id = deviceId('lbl');
    const pk = await createDevice(api, id);
    try {
      const devices = new DevicesPage(app);
      await devices.open();
      await devices.row(id).getByRole('button', { name: app.t('devices-detail'), exact: true }).click();

      const input = app.page.getByPlaceholder(app.t('resources-label-input-placeholder'));
      await input.fill('site:serre');
      await input.press('Enter');
      await app.page.getByRole('button', { name: app.t('resources-label-save'), exact: true }).click();
      await expect.poll(async () => (await api.get(`/resources/device/${pk}/labels`)).labels).toEqual({ site: 'serre' });
      await capture('device-labels', { caption: 'Labels of a device' });

      // The list filter keeps only the labelled device.
      await devices.open();
      const filter = app.page.getByPlaceholder(app.t('resources-label-filter-placeholder'));
      await filter.fill('site:serre');
      await filter.press('Enter');
      await expect(devices.row(id)).toBeVisible();
      const others = (await api.list('/devices?label=site:serre')).map((d: any) => d.device_id);
      expect(others).toContain(id);
    } finally {
      await api.delete(`/devices/${pk}`);
    }
  });

  test('location breadcrumb opens Sites on the site, folder unfolded', async ({ app, api, prefix, capture }) => {
    const id = deviceId('loc');
    const pk = await createDevice(api, id);
    const poi = await api.post('/pois', { label: `${prefix} plant`, emoji: '🔥', latitude: 46.6, longitude: 2.4 });
    try {
      const folder = await api.post('/resources/folders', { name: `${prefix} boiler room` });
      await api.put(`/resources/folder/${folder.id}/containment`, { parent: { kind: 'map_pin', id: poi.id } });
      // Same path as the UI: attached to the site first, then stored in the folder.
      await api.post(`/pois/${poi.id}/devices`, { device_id: id });
      await api.put(`/resources/device/${pk}/containment`, { parent: { kind: 'folder', id: String(folder.id) } });

      const devices = new DevicesPage(app);
      await devices.open();
      await devices.row(id).getByRole('button', { name: app.t('devices-detail'), exact: true }).click();
      const crumb = app.page.getByRole('navigation', { name: app.t('location-label') });
      await expect(crumb).toContainText(`${prefix} plant`);
      await expect(crumb).toContainText(`${prefix} boiler room`);
      await capture('location-breadcrumb', { caption: 'Location of a device' });

      await crumb.getByRole('button', { name: `${prefix} boiler room` }).click();
      await expect(app.page).toHaveURL(/\/sites$/);
      const drawer = app.page.locator('aside.right-0');
      await expect(drawer).toContainText(`${prefix} plant`);
      // The folder is unfolded: the device shows inside it.
      await expect(drawer.getByText(id, { exact: true })).toBeVisible();
      await capture('sites-drawer', { caption: 'Sites: the site opened from the breadcrumb' });

      // Sites lists the site; Map view and back.
      await app.page.keyboard.press('Escape');
      await app.goto('/sites');
      await expect(app.page.getByRole('main').getByText(`${prefix} plant`).first()).toBeVisible();
    } finally {
      await api.delete(`/devices/${pk}`);
      await api.delete(`/pois/${poi.id}`);
    }
  });

  test('board change keeps the device and moves the soldered screen', async ({ app, api }) => {
    const id = deviceId('brd');
    const pk = await createDevice(api, id);
    try {
      const devices = new DevicesPage(app);
      await devices.open();
      await devices.row(id).getByRole('button', { name: app.t('devices-detail'), exact: true }).click();
      await app.page.getByRole('button', { name: app.t('board-change-open'), exact: true }).click();
      const dlg = app.page.getByRole('dialog', { name: app.t('board-change-title') });
      await dlg.getByText(/NodeMCU V3.*OLED/).first().click();
      await dlg.getByRole('button', { name: app.t('board-change-confirm'), exact: true }).click();
      await expect(dlg).toBeHidden();
      await expect
        .poll(async () => (await api.get(`/devices/${pk}/pinout`)).board?.name)
        .toBe('nodemcu_v3_oled');
    } finally {
      await api.delete(`/devices/${pk}`);
    }
  });
});

test.describe('platform admin', { tag: '@settings' }, () => {
  test('subscription of an org changed from Platform status', async ({ app, api }) => {
    const rows: any[] = await api.get('/system/orgs');
    const org = rows.find((r) => r.org_id === api.orgId);
    const tiers: any[] = await api.get('/system/tiers');
    test.skip(!org || tiers.length === 0, 'needs a platform admin and at least one tier');
    const target = tiers.find((t) => t.id !== org.tier_id) ?? tiers[0];
    try {
      await app.goto('/admin/status');
      // The row of the test org, then its subscription list.
      const row = app.page.locator('main tr').filter({ has: app.page.getByText(org.name, { exact: true }) });
      await row.getByRole('combobox', { name: app.tr('admin-orgs-tier-label') }).selectOption(String(target.id));
      await expect
        .poll(async () => ((await api.get('/system/orgs')) as any[]).find((r) => r.org_id === api.orgId)?.tier_id)
        .toBe(target.id);
    } finally {
      await api.put(`/system/orgs/${api.orgId}/tier`, { tier_id: org?.tier_id ?? null });
    }
  });
});
