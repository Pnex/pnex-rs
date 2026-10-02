// Custom firmware on a real ESP32-C3: written in the IDE, attached at
// registration, built server-side, flashed, its own metric shows up. The
// board goes back to its generic e2e-c3 firmware afterwards.
import { writeFileSync } from 'node:fs';
import { expect, test } from '../src/fixtures.ts';
import type { Api } from '../src/api.ts';
import { BOARDS, WIFI, flashMerged, missingHardware } from '../src/hardware.ts';
import { DevicesPage } from '../src/pages/devices.ts';
import { dialog } from '../src/pages/shell.ts';

const board = BOARDS.c3;
const FW_DEVICE = 'e2e-c3-fw';
const METRIC = 'e2e_counter';

interface DeviceRow {
  id: number;
  device_id: string;
  connected: boolean;
}

async function device(api: Api, id: string): Promise<DeviceRow | undefined> {
  return (await api.list<DeviceRow>('/devices')).find((d) => d.device_id === id);
}

async function latestBuild(api: Api, id: string): Promise<string> {
  const rows = (await api.list<{ id: number; device_id: string | null; build_phase: string | null }>('/build-records'))
    .filter((b) => b.device_id === id)
    .sort((a, b) => b.id - a.id);
  return rows[0]?.build_phase ?? 'none';
}

test.describe('hardware custom firmware', { tag: '@hardware' }, () => {
  test('c3: IDE firmware built, flashed, publishes its metric; generic restored', async ({ app, api, page, prefix, capture }, info) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const generic = await device(api, board.deviceId);
    test.skip(!generic, `${board.deviceId} not registered (its generic image is restored at the end)`);
    test.setTimeout(30 * 60_000);

    // 1. Project: the starter sketch plus a counter metric.
    const project = `${prefix} counter`;
    await app.goto('/firmware');
    await page.getByRole('main').getByRole('button', { name: app.t('firmware-new') }).click();
    const create = dialog(page, app.t('firmware-create-title'));
    await create.getByRole('textbox', { name: app.t('firmware-field-name') }).fill(project);
    await create.getByRole('combobox', { name: new RegExp(`^${app.t('firmware-field-chip')}`) }).selectOption({ label: 'ESP32-C3' });
    await create.getByRole('button', { name: app.t('firmware-new'), exact: true }).click();
    const code = page.getByRole('main').locator('textarea').first();
    await expect(code).toHaveValue(/PnexDevice pnex;/);
    const sketch = (await code.inputValue())
      .replace('pnex.addMetric("uptime_s", "s");', `pnex.addMetric("uptime_s", "s");\n    pnex.addMetric("${METRIC}", "");`)
      .replace('pnex.publish("uptime_s", millis() / 1000UL);', `pnex.publish("uptime_s", millis() / 1000UL);\n        pnex.publish("${METRIC}", (long)(millis() / 10000UL));`);
    expect(sketch).toContain(METRIC);
    await code.fill(sketch);
    await page.getByRole('main').getByRole('button', { name: app.t('firmware-save'), exact: true }).click();
    await expect(page.getByRole('main').getByText(/\br2\b/)).toBeVisible();
    await capture('custom-firmware-code', { caption: 'Custom firmware with an extra metric' });

    // 2. Register a device running it, build, flash.
    const old = await device(api, FW_DEVICE);
    if (old) await api.delete(`/devices/${old.id}`);
    const devices = new DevicesPage(app);
    await devices.open();
    await devices.register({ deviceId: FW_DEVICE, model: board.model, wifi: WIFI, firmware: project });
    await expect.poll(() => latestBuild(api, FW_DEVICE), { timeout: 15 * 60_000, intervals: [5_000] }).toMatch(/succeeded|failed/);
    expect(await latestBuild(api, FW_DEVICE)).toBe('succeeded');
    const image = info.outputPath(`${FW_DEVICE}.bin`);
    writeFileSync(image, await api.download(`/download/firmware/${FW_DEVICE}`));
    await flashMerged(board, image);

    try {
      await expect.poll(async () => (await device(api, FW_DEVICE))?.connected ?? false, { timeout: 3 * 60_000, intervals: [5_000] }).toBe(true);
      // 3. Its own metric reaches the telemetry store.
      await expect
        .poll(
          async () => (await api.get<{ points: unknown[] }>(`/telemetry/series?metric=${METRIC}&device_id=${FW_DEVICE}&window=5m`)).points.length,
          { timeout: 3 * 60_000, intervals: [10_000], message: `${METRIC} published` },
        )
        .toBeGreaterThan(0);
    } finally {
      // 4. Back to the generic firmware already built for e2e-c3.
      const restore = info.outputPath(`${board.deviceId}.bin`);
      writeFileSync(restore, await api.download(`/download/firmware/${board.deviceId}`));
      await flashMerged(board, restore);
      const fw = await device(api, FW_DEVICE);
      if (fw) await api.delete(`/devices/${fw.id}`);
    }
    await expect.poll(async () => (await device(api, board.deviceId))?.connected ?? false, { timeout: 3 * 60_000, intervals: [5_000] }).toBe(true);
  });
});
