// Native Linux app under test (Dioxus desktop on WebKitGTK), driven through
// WebKitWebDriver (apt `webkit2gtk-driver`) + WebdriverIO. Playwright cannot
// attach to WebKitGTK: the runner stays Playwright, the driver is WebDriver.
//
// Each test gets its own XDG_DATA_HOME: the app's storage
// (`<XDG_DATA_HOME>/pnex/pnex-storage.json`) is seeded there before launch,
// the operator's own data is never touched.
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { remote } from 'webdriverio';
import { REPO_ROOT } from './env.ts';

/** e2e build of the app (`task build:frontend:linux:e2e`). */
export const APP =
  process.env.PNEX_E2E_LINUX_APP ?? path.join(REPO_ROOT, 'target/dx/pnex-frontend/linux-e2e/pnex-frontend');
export const DRIVER = process.env.PNEX_E2E_WEBKIT_DRIVER ?? 'WebKitWebDriver';

export type Browser = WebdriverIO.Browser;

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.listen(0, '127.0.0.1', () => {
      const port = (server.address() as net.AddressInfo).port;
      server.close(() => resolve(port));
    });
    server.on('error', reject);
  });
}

async function waitForPort(port: number, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const ok = await new Promise<boolean>((resolve) => {
      const socket = net.connect(port, '127.0.0.1', () => {
        socket.end();
        resolve(true);
      });
      socket.on('error', () => resolve(false));
    });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`${DRIVER} did not listen on :${port}`);
}

/** One launch of the app: its driver, data dir and WebDriver session. */
export class LinuxApp {
  private constructor(
    readonly browser: Browser,
    readonly dataHome: string,
    private readonly driver: ChildProcess,
    private readonly output: string[],
  ) {}

  /** Rust panics the app printed (it inherits the driver's stderr). */
  panics(): string[] {
    return this.output.join('').split('\n').filter((l) => /panicked at|PNEX-PANIC/.test(l));
  }

  static async launch(storage: Record<string, string>, env: Record<string, string> = {}): Promise<LinuxApp> {
    const dataHome = mkdtempSync(path.join(os.tmpdir(), 'pnex-e2e-linux-'));
    if (Object.keys(storage).length) {
      mkdirSync(path.join(dataHome, 'pnex'), { recursive: true });
      writeFileSync(path.join(dataHome, 'pnex', 'pnex-storage.json'), JSON.stringify(storage));
    }
    const port = await freePort();
    // The driver spawns the app: the app inherits this environment.
    const driver = spawn(DRIVER, [`--port=${port}`], {
      env: { ...process.env, ...env, PNEX_E2E_AUTOMATION: '1', XDG_DATA_HOME: dataHome },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const output: string[] = [];
    driver.stdout?.on('data', (d) => output.push(String(d)));
    driver.stderr?.on('data', (d) => output.push(String(d)));
    try {
      await waitForPort(port);
      const browser = await remote({
        hostname: '127.0.0.1',
        port,
        logLevel: 'error',
        capabilities: {
          // @ts-expect-error vendor capability of WebKitWebDriver
          'webkitgtk:browserOptions': { binary: APP, args: ['--automation'] },
        },
      });
      // Desktop width: dx opens an 800 px window, below `lg`, where some
      // pages fold their side panel (the map's title lives in it).
      await browser.setWindowSize(1280, 800).catch(() => {});
      return new LinuxApp(browser, dataHome, driver, output);
    } catch (e) {
      driver.kill();
      rmSync(dataHome, { recursive: true, force: true });
      throw e;
    }
  }

  readStorage(): Record<string, string> {
    try {
      return JSON.parse(readFileSync(path.join(this.dataHome, 'pnex', 'pnex-storage.json'), 'utf8'));
    } catch {
      return {};
    }
  }

  async close(): Promise<void> {
    await this.browser.deleteSession().catch(() => {});
    this.driver.kill();
    rmSync(this.dataHome, { recursive: true, force: true });
  }
}

/** UI locale a fresh install picks on this machine (POSIX precedence). */
export function systemLocale(): string {
  const tag = [process.env.LC_ALL, process.env.LC_MESSAGES, process.env.LANG].find(
    (v) => v && v !== 'C' && v !== 'POSIX',
  );
  return tag?.toLowerCase().startsWith('fr') ? 'fr-FR' : 'en-US';
}
