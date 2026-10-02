// Real boards: register through the UI wizard, build server-side, flash the
// merged image with esptool on the declared port, then watch the device
// come online. Skipped unless the boards and Wi-Fi are declared (README).
import { writeFileSync } from 'node:fs';
import { expect, test } from '../../src/fixtures.ts';
import type { Api } from '../../src/api.ts';
import { BOARDS, WIFI, flashMerged, missingHardware, type Board } from '../../src/hardware.ts';
import { DevicesPage } from '../../src/pages/devices.ts';

interface DeviceRow {
  id: number;
  device_id: string;
  connected: boolean;
}

interface BuildRow {
  id: number;
  device_id: string | null;
  build_phase: string | null;
}

async function deviceRow(api: Api, deviceId: string): Promise<DeviceRow | undefined> {
  return (await api.list<DeviceRow>('/devices')).find((d) => d.device_id === deviceId);
}

async function waitBuild(api: Api, deviceId: string): Promise<BuildRow> {
  let last: BuildRow | undefined;
  await expect
    .poll(
      async () => {
        last = (await api.list<BuildRow>('/build-records'))
          .filter((b) => b.device_id === deviceId)
          .sort((a, b) => b.id - a.id)[0];
        return last?.build_phase ?? 'none';
      },
      { timeout: 15 * 60_000, intervals: [5_000], message: `firmware build of ${deviceId}` },
    )
    .toMatch(/succeeded|failed/);
  expect(last?.build_phase, `build of ${deviceId}`).toBe('succeeded');
  return last!;
}

export function registerAndFlash(key: keyof typeof BOARDS) {
  const board: Board = BOARDS[key];
  test(`${key}: register, build, flash, online`, async ({ app, api, capture }, info) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    test.setTimeout(25 * 60_000);

    // Fresh registration every run: the wizard is part of the test.
    const existing = await deviceRow(api, board.deviceId);
    if (existing) await api.delete(`/devices/${existing.id}`);

    const devices = new DevicesPage(app);
    await devices.open();
    await devices.register({ deviceId: board.deviceId, model: board.model, wifi: WIFI });
    await capture('register-review', { caption: 'Registration: firmware build started' });

    await waitBuild(api, board.deviceId);
    const image = info.outputPath(`${board.deviceId}.bin`);
    writeFileSync(image, await api.download(`/download/firmware/${board.deviceId}`));
    await flashMerged(board, image);

    await expect
      .poll(async () => (await deviceRow(api, board.deviceId))?.connected ?? false, {
        timeout: 3 * 60_000,
        intervals: [5_000],
        message: `${board.deviceId} online`,
      })
      .toBe(true);

    await devices.open();
    await expect(devices.row(board.deviceId)).toBeVisible();
    await capture('device-online', { caption: 'Device online after flashing' });
  });
}
