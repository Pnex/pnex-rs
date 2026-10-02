// Live ESP32-CAM: the live dialog receives fresh frames, and the flash LED
// is switched from the UI. Needs the camera registered and online
// (hardware.spec.ts "cam"); skipped otherwise.
import { expect, test } from '../src/fixtures.ts';
import { BOARDS, missingHardware } from '../src/hardware.ts';
import { dialog } from '../src/pages/shell.ts';

const board = BOARDS.cam;

test.describe('hardware camera', { tag: '@hardware' }, () => {
  test('cam: live frames and flash switch', async ({ app, api, page, capture }, info) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const online = (await api.list<{ device_id: string; connected: boolean }>('/devices')).some(
      (d) => d.device_id === board.deviceId && d.connected,
    );
    test.skip(!online, `${board.deviceId} not online (run the hardware registration first)`);
    test.setTimeout(2 * 60_000);

    await app.goto('/cameras');
    const row = page.locator('main tr').filter({ hasText: board.deviceId });
    await row.getByRole('button', { name: app.t('cameras-live-start'), exact: true }).click();
    const live = dialog(page, app.tr('cameras-live-title'));
    const frame = live.locator('img[src^="blob:"]');
    await expect(frame).toBeVisible({ timeout: 30_000 });

    // Stream rate over 20 s from the bridge's frame counter, against the
    // configured fps (the row shows "… · 5 fps · …").
    const counter = live.locator('[data-pnex-frames]');
    const frames = async () => Number((await counter.getAttribute('data-pnex-frames')) ?? 0);
    const fps = Number((await row.innerText()).match(/(\d+) fps/)?.[1] ?? 5);
    const before = await frames();
    await page.waitForTimeout(20_000);
    const received = (await frames()) - before;
    info.annotations.push({ type: 'stream', description: `${received} frames in 20 s (configured ${fps} fps)` });
    expect(received, `live frames in 20 s at ${fps} fps`).toBeGreaterThanOrEqual(fps * 20 * 0.5);
    await capture('camera-live', { caption: 'Live view of the ESP32-CAM', target: live });

    // Flash LED on, then off again.
    await live.getByRole('switch', { name: app.t('cameras-flash-turn-on') }).click();
    await expect(live.getByRole('switch', { name: app.t('cameras-flash-turn-off') })).toBeVisible({ timeout: 15_000 });
    await live.getByRole('switch', { name: app.t('cameras-flash-turn-off') }).click();
    await expect(live.getByRole('switch', { name: app.t('cameras-flash-turn-on') })).toBeVisible({ timeout: 15_000 });
    await live.getByRole('button', { name: app.t('common-close') }).click();
  });
});
