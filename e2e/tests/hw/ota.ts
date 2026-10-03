// Over-the-air update of a live board (C3, C6): rebuild from the device row,
// deploy over the air, the board downloads, flashes, reboots and reports
// success. The build record is reused by a rebuild (same id = same
// firmware version), so the deployment is a forced redeploy.
import { expect, test } from '../../src/fixtures.ts';
import type { Api } from '../../src/api.ts';
import { BOARDS, WIFI, missingHardware, type Board } from '../../src/hardware.ts';
import { DevicesPage } from '../../src/pages/devices.ts';

interface DeviceRow {
  id: number;
  device_id: string;
  connected: boolean;
  fw_version: string | null;
}

interface BuildRow {
  id: number;
  device_id: string | null;
  build_phase: string | null;
  updated_at: string;
}

interface OtaRow {
  id: number;
  state: string;
  target_version: string;
  progress: number | null;
  error: string | null;
}

async function device(api: Api, board: Board): Promise<DeviceRow | undefined> {
  return (await api.list<DeviceRow>('/devices')).find((d) => d.device_id === board.deviceId);
}

async function build(api: Api, board: Board): Promise<BuildRow | undefined> {
  return (await api.list<BuildRow>('/build-records')).filter((b) => b.device_id === board.deviceId).sort((a, b) => b.id - a.id)[0];
}

export function otaTests(key: keyof typeof BOARDS): void {
  const board: Board = BOARDS[key];
  test(`${key}: rebuild then update over the air`, async ({ app, api, capture }) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const before = await device(api, board);
    test.skip(!before?.connected, `${board.deviceId} not online (run the hardware registration first)`);
    test.setTimeout(20 * 60_000);

    const devices = new DevicesPage(app);
    await devices.open();
    const clickedAt = Date.now();
    await devices.rebuild(board.deviceId, WIFI);
    await expect
      .poll(
        async () => {
          const b = await build(api, board);
          return b && Date.parse(b.updated_at) >= clickedAt - 2_000 ? b.build_phase : 'stale';
        },
        { timeout: 15 * 60_000, intervals: [5_000], message: 'rebuild' },
      )
      .toMatch(/succeeded|failed/);
    expect((await build(api, board))?.build_phase).toBe('succeeded');

    const previous = (await api.get<{ history: OtaRow[] }>(`/devices/${before!.id}/ota`)).history[0]?.id ?? 0;
    await devices.open();
    await devices.deployOta(board.deviceId);
    await capture('ota-deploy', { caption: 'Over-the-air update in progress' });

    // pending → downloading → flashing → succeeded (reported after reboot).
    let last: OtaRow | undefined;
    await expect
      .poll(
        async () => {
          last = (await api.get<{ history: OtaRow[] }>(`/devices/${before!.id}/ota`)).history.find((r) => r.id > previous);
          return last?.state ?? 'none';
        },
        { timeout: 6 * 60_000, intervals: [5_000], message: 'OTA assignment' },
      )
      .toMatch(/succeeded|failed/);
    expect(last?.error ?? null, 'OTA error').toBeNull();
    expect(last?.state).toBe('succeeded');
    await expect.poll(async () => (await device(api, board))?.connected ?? false, { timeout: 2 * 60_000 }).toBe(true);
  });
}
