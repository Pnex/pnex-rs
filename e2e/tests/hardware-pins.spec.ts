// Pins of a live ESP32-C3 driven from the device page: output mode + write,
// analog input subscribed at 1 s. Needs the board registered and online
// (hardware.spec.ts "c3"); skipped otherwise.
import { expect, test } from '../src/fixtures.ts';
import type { Api } from '../src/api.ts';
import { BOARDS, missingHardware } from '../src/hardware.ts';
import { DevicesPage } from '../src/pages/devices.ts';

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

test.describe('hardware pins', { tag: '@hardware' }, () => {
  test.describe.configure({ mode: 'serial' });

  test('c3: output mode, write HIGH/LOW, analog input subscribed', async ({ app, api, capture }) => {
    const missing = missingHardware(board);
    test.skip(!!missing, missing);
    const pk = await devicePk(api);
    test.skip(pk == null, `${board.deviceId} not online (run the hardware registration first)`);
    test.setTimeout(3 * 60_000);

    const devices = new DevicesPage(app);
    await devices.open();
    await devices.openDetail(board.deviceId);

    // Output: mode applied on the board, then manual writes.
    const out = await devices.openPin(OUT_GPIO);
    await out.setMode('digital_out');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.mode, { timeout: 30_000 }).toBe('digital_out');
    await out.write('high');
    await capture('pin-output-high', { caption: 'D2 switched to output and written HIGH' });
    await out.write('low');
    await out.close();

    // Analog input read every second: values show up in the pin state.
    const adc = await devices.openPin(ADC_GPIO);
    await adc.setMode('analog_in');
    await expect.poll(async () => (await pin(api, pk!, ADC_GPIO))?.mode, { timeout: 30_000 }).toBe('analog_in');
    await adc.subscribe(1000);
    await expect
      .poll(async () => typeof (await pin(api, pk!, ADC_GPIO))?.last_value, { timeout: 30_000, message: 'ADC value reported' })
      .toBe('number');
    await capture('pin-analog-subscribed', { caption: 'D0 read every second' });

    // Back to the defaults so the next run starts from the same state.
    await adc.subscribe(0);
    await adc.setMode('digital_in');
    await adc.close();
    const reset = await devices.openPin(OUT_GPIO);
    await reset.setMode('digital_in');
    await expect.poll(async () => (await pin(api, pk!, OUT_GPIO))?.mode, { timeout: 30_000 }).toBe('digital_in');
  });
});
