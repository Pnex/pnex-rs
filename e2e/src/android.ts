// Native Android app under test: adb plumbing, session seeding and the
// WebView page Playwright drives (`_android` + the app's DevTools socket).
//
// The app keeps its storage in `files/pnex-storage.json` (not localStorage):
// the session is written there through `run-as`, which needs a debuggable
// APK — the e2e APK (`task build:frontend:android:e2e`) is one.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { _android, type AndroidDevice, type Page } from 'playwright';
import { REPO_ROOT } from './env.ts';

export const PACKAGE = 'io.pnex.app';
export const ACTIVITY = `${PACKAGE}/dev.dioxus.main.MainActivity`;
/** Origin the app's WebView serves the bundle from (wry custom protocol). */
export const APP_ORIGIN = 'https://dioxus.index.html';

/** Device serial (`adb devices`); unset = the only device attached. */
export const SERIAL = process.env.PNEX_E2E_ANDROID_SERIAL;
/** APK installed before the run when set to `1` (else: already installed). */
export const INSTALL = process.env.PNEX_E2E_ANDROID_INSTALL === '1';
export const APK =
  process.env.PNEX_E2E_ANDROID_APK ?? path.join(REPO_ROOT, 'target/dx/pnex-frontend/pnex-e2e.apk');

export function adb(args: string[], opts: { input?: string; allowFail?: boolean } = {}): string {
  const full = SERIAL ? ['-s', SERIAL, ...args] : args;
  try {
    return execFileSync('adb', full, { encoding: 'utf8', input: opts.input, stdio: ['pipe', 'pipe', 'pipe'] });
  } catch (e) {
    if (opts.allowFail) return String((e as { stdout?: string }).stdout ?? '');
    throw e;
  }
}

export function shell(cmd: string, allowFail = false): string {
  return adb(['shell', cmd], { allowFail }).trim();
}

export function appPid(): number | undefined {
  const out = shell(`pidof ${PACKAGE}`, true);
  return out ? Number(out.split(/\s+/)[0]) : undefined;
}

/** Window holding the input focus (`mCurrentFocus`), e.g. the camera app. */
export function focusedWindow(): string {
  const m = shell('dumpsys window', true).match(/mCurrentFocus=Window\{[^ ]+ [^ ]+ ([^}]+)\}/);
  return m?.[1] ?? '';
}

/**
 * Can the shell inject input events (taps, keys)? MIUI refuses them unless
 * "USB debugging (Security settings)" is on: native-UI steps (camera app,
 * system dialogs) are skipped instead of failing.
 */
export function canInjectInput(): boolean {
  const out = adb(['shell', 'input keyevent 0 2>&1'], { allowFail: true });
  return !/INJECT_EVENTS|SecurityException/.test(out);
}

export function cameraGranted(): boolean {
  return /android\.permission\.CAMERA: granted=true/.test(shell(`dumpsys package ${PACKAGE}`, true));
}

export function deviceLocale(): string {
  return shell('getprop persist.sys.locale', true) || shell('getprop ro.product.locale', true);
}

export function installApk(): void {
  adb(['install', '-r', APK]);
}

export function forceStop(): void {
  shell(`am force-stop ${PACKAGE}`, true);
}

export function launch(): void {
  shell(`am start -n ${ACTIVITY}`);
}

/** Clears logcat so `panics()` only sees this test's output. */
export function clearLogcat(): void {
  adb(['logcat', '-c'], { allowFail: true });
}

/** Rust panics logged by the app since the last `clearLogcat()`. */
export function panics(): string[] {
  const log = adb(['logcat', '-d', '-v', 'brief'], { allowFail: true });
  return log.split('\n').filter((l) => /PNEX-PANIC|panicked at/.test(l));
}

/**
 * Replaces the app's storage with `entries` (app stopped first: it caches the
 * file in memory and would write its own copy back).
 */
export function writeStorage(entries: Record<string, string>): void {
  forceStop();
  const dir = mkdtempSync(path.join(os.tmpdir(), 'pnex-e2e-'));
  const local = path.join(dir, 'pnex-storage.json');
  const remote = '/data/local/tmp/pnex-e2e-storage.json';
  try {
    writeFileSync(local, JSON.stringify(entries));
    adb(['push', local, remote]);
    shell(`run-as ${PACKAGE} sh -c 'mkdir -p files && cp ${remote} files/pnex-storage.json'`);
  } finally {
    shell(`rm -f ${remote}`, true);
    rmSync(dir, { recursive: true, force: true });
  }
}

export function readStorage(): Record<string, string> {
  const raw = shell(`run-as ${PACKAGE} cat files/pnex-storage.json`, true);
  try {
    return JSON.parse(raw);
  } catch {
    return {};
  }
}

export async function connectDevice(): Promise<AndroidDevice> {
  const devices = await _android.devices();
  const device = SERIAL ? devices.find((d) => d.serial() === SERIAL) : devices[0];
  if (!device) throw new Error(`no Android device${SERIAL ? ` with serial ${SERIAL}` : ''} (adb devices)`);
  return device;
}

/** Launches the app and returns its WebView page once the bundle is up. */
export async function openApp(device: AndroidDevice): Promise<Page> {
  launch();
  // Target this process' DevTools socket: a lookup by package can still
  // return the WebView of the process stopped by the previous test.
  let pid: number | undefined;
  for (let i = 0; i < 60 && !pid; i++) {
    pid = appPid();
    if (!pid) await new Promise((r) => setTimeout(r, 250));
  }
  if (!pid) throw new Error(`${PACKAGE} did not start`);
  const webView = await device.webView({ socketName: `webview_devtools_remote_${pid}` }, { timeout: 30_000 });
  const page = await webView.page();
  await page.waitForLoadState('domcontentloaded');
  return page;
}
