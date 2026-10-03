// Pins of a live ESP32-C3 driven from the device page: output mode + write,
// analog input subscribed at 1 s. Needs the board registered and online
// (hardware.spec.ts "c3"); skipped otherwise. Values are checked as the
// board measured them (D122): pad read back after each write, in the API
// and in the firmware serial log.
import { expect, test } from '../../src/fixtures.ts';
import type { Api } from '../../src/api.ts';
import { BOARDS, missingHardware, serialCapture } from '../../src/hardware.ts';
import { DevicesPage } from '../../src/pages/devices.ts';
import { fieldAfterLabel } from '../../src/pages/shell.ts';

interface PinRow {
  gpio: number;
  label: string;
  mode: string;
  last_value?: unknown;
  interval_ms?: number;
}

const board = BOARDS.c3;
/** XIAO ESP32-C3: D2 = GPIO4 (output), D0 = GPIO2 (ADC1). */
const OUT_GPIO = 4;
const ADC_GPIO = 2;

async function devicePk(api: Api): Promise<number | undefined> {
  const rows = await api.list<{ id: number; device_id: string; connected: boolean }>('/devices');
  return rows.find((d) => d.device_id === board.deviceId && d.connected)?.id;
}

async function pin(api: Api, pk: number, gpio: number): Promise<PinRow | undefined> {
  const res = await api.get<{ pins: PinRow[] }>(`/devices/${pk}/pins`);
  return res.pins.find((p) => p.gpio === gpio);
}

export function pinsTests(): void {
  test('c3: output mode, write HIGH/LOW, analog input subscribed', async ({ app, api, capture }) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const pk = await devicePk(api);
    test.skip(pk == null, `${board.deviceId} not online (run the hardware registration first)`);
    test.setTimeout(3 * 60_000);

    const devices = new DevicesPage(app);
    await devices.open();
    await devices.openDetail(board.deviceId);

    const serial = serialCapture(board, 90);

    // Output: mode applied on the board, then manual writes; the value the
    // server stores is the level read back on the pad.
    const out = await devices.openPin(OUT_GPIO);
    await out.setMode('digital_out');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.mode, { timeout: 30_000 }).toBe('digital_out');
    await out.write('high');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.last_value, { timeout: 30_000 }).toBe(true);
    await capture('pin-output-high', { caption: 'D2 switched to output and written HIGH' });
    await out.write('low');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.last_value, { timeout: 30_000 }).toBe(false);
    await out.close();
    await expect
      .poll(() => serial.text(), { timeout: 15_000, message: 'serial readback after the HIGH write' })
      .toContain(`write GPIO${OUT_GPIO} -> HIGH (readback=HIGH)`);
    await expect.poll(() => serial.text(), { timeout: 15_000 }).toContain(`write GPIO${OUT_GPIO} -> LOW (readback=LOW)`);
    expect(serial.text()).not.toContain('[IO] MISMATCH');

    // Analog input read every second: values show up in the pin state.
    const adc = await devices.openPin(ADC_GPIO);
    await adc.setMode('analog_in');
    await expect.poll(async () => (await pin(api, pk!, ADC_GPIO))?.mode, { timeout: 30_000 }).toBe('analog_in');
    await adc.subscribe(1000);
    await expect
      .poll(async () => typeof (await pin(api, pk!, ADC_GPIO))?.last_value, { timeout: 30_000, message: 'ADC value reported' })
      .toBe('number');
    // The raw count the server received is also on the serial log.
    await expect
      .poll(() => serial.text(), { timeout: 15_000, message: 'ADC value in the serial log' })
      .toMatch(new RegExp(`\\[IO\\] GPIO${ADC_GPIO} mode=adc_in value=\\d+`));
    await capture('pin-analog-subscribed', { caption: 'D0 read every second' });

    await serial.stop();

    // Back to the defaults so the next run starts from the same state.
    await adc.subscribe(0);
    await adc.setMode('digital_in');
    await adc.close();
    const reset = await devices.openPin(OUT_GPIO);
    await reset.setMode('digital_in');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.mode, { timeout: 30_000 }).toBe('digital_in');
  });

  test('c3: subscribed analog input reaches the telemetry store and quick charts', async ({ app, api, page, capture }) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const pk = await devicePk(api);
    test.skip(pk == null, `${board.deviceId} not online (run the hardware registration first)`);
    test.setTimeout(5 * 60_000);

    const devices = new DevicesPage(app);
    await devices.open();
    await devices.openDetail(board.deviceId);
    const adc = await devices.openPin(ADC_GPIO);
    await adc.setMode('analog_in');
    await expect.poll(async () => (await pin(api, pk!, ADC_GPIO))?.mode, { timeout: 30_000 }).toBe('analog_in');
    await adc.subscribe(1000);
    const since = Date.now() / 1000;

    // Telemetry is batched to the store: wait for a point newer than the
    // subscription (5 min window, buckets of a few seconds).
    const metric = 'd0';
    await expect
      .poll(
        async () => {
          const res = await api.get<{ points: { ts: number }[] }>(`/telemetry/series?metric=${metric}&device_id=${board.deviceId}&window=5m`);
          return res.points.filter((p) => p.ts >= since - 10).length;
        },
        { timeout: 3 * 60_000, intervals: [10_000], message: `${metric} points since the subscription` },
      )
      .toBeGreaterThan(0);

    // Quick charts: pick the series, add it, a curve is drawn.
    await app.goto('/visualisation');
    const main = page.getByRole('main');
    await fieldAfterLabel(main, app.t('vis-metric')).selectOption(metric);
    await fieldAfterLabel(main, app.t('vis-device')).selectOption(board.deviceId);
    await main.getByRole('button', { name: app.t('vis-add'), exact: true }).click();
    await expect(main.getByText(app.t('vis-empty'))).toHaveCount(0);
    await expect(main.getByText(app.t('vis-no-points'))).toHaveCount(0);
    await expect(main.locator('svg path, canvas').first()).toBeVisible();
    await capture('quick-charts', { caption: 'Live analog input in quick charts' });

    await adc.root.page().goto('/devices');
    await devices.openDetail(board.deviceId);
    const reset = await devices.openPin(ADC_GPIO);
    await reset.subscribe(0);
    await reset.setMode('digital_in');
  });
}
