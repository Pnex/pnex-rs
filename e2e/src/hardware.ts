// Boards on serial ports, for tests tagged @hardware.
//
// Flashing always names its port: esptool without --port writes to the
// first board it finds (a bootloader was lost that way once).
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { promisify } from 'node:util';

const run = promisify(execFile);

export interface Board {
  /** Serial port, from the environment. */
  port: string;
  /** Wizard model button label prefix ("Generic ESP32-C3"). */
  model: RegExp;
  /** device_id registered for this board in the test org (≤ 16 chars). */
  deviceId: string;
  chip: string;
  /** Upload baud rate (CH340 carriers are flaky above 460800). */
  baud: number;
  /** Wizard board variant chip (pretty name); the model default when absent. */
  variant?: string;
  /** Wizard debug screen button label (e.g. `TFT 1.77"`); none when absent. */
  screen?: string;
  /** GPIOs the picked screen reserves (never provisioned as pins). */
  screenGpios?: number[];
  /** IDE chip family label (custom firmware projects). */
  chipLabel?: string;
}

export const BOARDS = {
  c3: {
    port: process.env.PNEX_E2E_C3_PORT ?? '',
    model: /^Generic ESP32-C3/,
    deviceId: process.env.PNEX_E2E_C3_ID ?? 'e2e-c3',
    chip: 'esp32c3',
    baud: 460800,
    chipLabel: 'ESP32-C3',
  },
  c6: {
    port: process.env.PNEX_E2E_C6_PORT ?? '',
    model: /^Generic ESP32-C6/,
    deviceId: process.env.PNEX_E2E_C6_ID ?? 'e2e-c6',
    chip: 'esp32c6',
    baud: 460800,
    chipLabel: 'ESP32-C6',
  },
  cam: {
    port: process.env.PNEX_E2E_CAM_PORT ?? '',
    model: /^Generic ESP32-CAM/,
    deviceId: process.env.PNEX_E2E_CAM_ID ?? 'e2e-cam',
    chip: 'esp32',
    baud: 460800,
  },
  esp32: {
    port: process.env.PNEX_E2E_ESP32_PORT ?? '',
    model: /^Generic ESP32 \(DevKit/,
    deviceId: process.env.PNEX_E2E_ESP32_ID ?? 'e2e-esp32',
    chip: 'esp32',
    baud: 460800,
    variant: 'ESP32 DevKit 38 pins (TXD/RXD)',
    screen: 'TFT 1.77"',
    // ST7735 on VSPI: SCK 18, MOSI 23, CS 5, DC 2, RST 4.
    screenGpios: [18, 23, 5, 2, 4],
  },
  nodemcu: {
    port: process.env.PNEX_E2E_NODEMCU_PORT ?? '',
    model: /^Generic ESP8266/,
    deviceId: process.env.PNEX_E2E_NODEMCU_ID ?? 'e2e-nodemcu',
    chip: 'esp8266',
    baud: 460800,
    variant: 'NodeMCU V3 + OLED 0.96" (soldered, CH340G)',
    // Soldered OLED (builtin, forced): SDA D6 = GPIO12, SCL D5 = GPIO14.
    screenGpios: [12, 14],
  },
} satisfies Record<string, Board>;

export const WIFI = {
  ssid: process.env.PNEX_E2E_WIFI_SSID ?? '',
  password: process.env.PNEX_E2E_WIFI_PASSWORD ?? '',
};

/** Reason to skip, or undefined when the board and Wi-Fi are declared. */
export function missingHardware(board: Board): string | undefined {
  if (!board.port) return `no serial port declared for ${board.deviceId}`;
  if (!existsSync(board.port)) return `${board.port} not present`;
  if (!WIFI.ssid || !WIFI.password) return 'PNEX_E2E_WIFI_SSID / PNEX_E2E_WIFI_PASSWORD not set';
  return undefined;
}

/** Writes a merged image at 0x0 with the web flasher's parameters. */
export async function flashMerged(board: Board, image: string): Promise<string> {
  const { stdout, stderr } = await run(
    'esptool',
    [
      '--port', board.port,
      '--chip', board.chip,
      '--baud', String(board.baud),
      'write-flash',
      '--flash-mode', 'dio',
      '--flash-freq', '40m',
      '--flash-size', '4MB',
      '0x0', image,
    ],
    { timeout: 240_000, maxBuffer: 16 << 20 },
  );
  const out = stdout + stderr;
  if (!/Hash of data verified/.test(out)) throw new Error(`flash not verified on ${board.port}:\n${out}`);
  return out;
}

/** Running capture of a board's serial log (firmware/hil/serial_tail.py). */
export interface SerialCapture {
  /** Everything received so far. */
  text(): string;
  /** Resolves with the full log once the capture window ends. */
  done: Promise<string>;
  /** Ends the capture early and frees the port (flashing needs it). */
  stop(): Promise<string>;
}

/**
 * Tails the board's serial port for `seconds` without resetting it, so a
 * test can assert the real pin values the firmware logs (`[IO] ...`,
 * `readback=`). Released DTR/RTS keep the running firmware alive.
 */
export function serialCapture(board: Board, seconds: number): SerialCapture {
  let log = '';
  const child = execFile(
    'uv',
    ['run', '--project', '../firmware', 'python', '../firmware/hil/serial_tail.py', '--port', board.port, '--seconds', String(seconds)],
    { timeout: (seconds + 60) * 1000, maxBuffer: 16 << 20 },
  );
  child.stdout?.on('data', (chunk: Buffer | string) => {
    log += chunk.toString();
  });
  const done = new Promise<string>((resolve, reject) => {
    child.on('error', reject);
    child.on('close', () => resolve(log));
  });
  return {
    text: () => log,
    done,
    stop: () => {
      child.kill('SIGINT');
      return done;
    },
  };
}
