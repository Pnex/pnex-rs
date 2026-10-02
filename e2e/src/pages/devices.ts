// Devices page (/devices): list, registration wizard, device detail.
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, dialog } from './shell.ts';

export interface RegisterOptions {
  deviceId: string;
  /** Model button label (accessible name prefix). */
  model: RegExp;
  wifi: { ssid: string; password: string };
  /** Custom firmware project name (generic firmware when absent). */
  firmware?: string;
}

export class DevicesPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  async open(): Promise<void> {
    await this.app.goto('/devices');
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-devices'));
  }

  row(deviceId: string): Locator {
    return this.page.locator('main tr').filter({ hasText: deviceId });
  }

  get wizard(): Locator {
    return dialog(this.page, this.app.t('devices-register-title'));
  }

  private next(): Promise<void> {
    return this.wizard.getByRole('button', { name: this.app.t('wizard-next'), exact: true }).click();
  }

  /**
   * Runs the registration wizard up to "Create & build". Adds the Wi-Fi
   * credential inline when the org has none; keeps the deployment's server.
   */
  async register(opts: RegisterOptions): Promise<void> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('devices-register'), exact: true }).click();
    const w = this.wizard;
    await w.getByRole('textbox', { name: this.app.t('devices-new-placeholder') }).fill(opts.deviceId);
    await this.next();

    await w.getByRole('button', { name: opts.model }).click();
    if (opts.firmware) {
      // Options read "<project> (r<rev>)"; the generic firmware is the empty value.
      const pick = w.getByRole('combobox').filter({ has: this.page.locator('option', { hasText: this.app.t('wizard-firmware-generic') }) });
      const option = pick.locator('option').filter({ hasText: opts.firmware });
      await pick.selectOption((await option.getAttribute('value'))!);
    }
    await this.next();

    // Wi-Fi: inline form when no credential exists yet, else a select
    // (rendered once the referentials are loaded).
    const ssid = w.getByRole('textbox', { name: this.app.t('builds-field-ssid') });
    await expect(ssid.or(w.getByRole('combobox').first())).toBeVisible();
    if (await ssid.isVisible()) {
      await ssid.fill(opts.wifi.ssid);
      await w.getByRole('textbox', { name: this.app.t('builds-field-wifi-password') }).fill(opts.wifi.password);
      await w.getByRole('button', { name: this.app.t('wizard-ref-add'), exact: true }).first().click();
      await expect(ssid).toBeHidden();
    }
    // PNeX server: the add form comes pre-filled with this deployment's
    // address when the org has no host yet — confirm it.
    const host = w.getByPlaceholder(this.app.t('builds-field-server'));
    if (await host.isVisible()) {
      await expect(host).not.toHaveValue('');
      await w.getByRole('button', { name: this.app.t('wizard-ref-add'), exact: true }).last().click();
      await expect(host).toBeHidden();
    }
    await this.next();
    await expect(w.getByText(this.app.t('wizard-review-build-note'))).toBeVisible();
    await w.getByRole('button', { name: this.app.t('wizard-create-build'), exact: true }).click();
  }

  async openDetail(deviceId: string): Promise<void> {
    await this.row(deviceId).getByRole('button', { name: this.app.t('devices-detail') }).click();
  }

  /** Pin drawer of the detail page, opened by clicking the GPIO chip. */
  async openPin(gpio: number): Promise<PinDrawer> {
    const chip = this.page.getByText(`GPIO${gpio}`, { exact: true }).first();
    await chip.waitFor();
    await chip.click();
    const drawer = this.page.getByRole('complementary').filter({ hasText: `GPIO${gpio}` }).last();
    await expect(drawer.getByRole('combobox', { name: this.app.t('pins-mode') }).or(drawer.getByRole('combobox', { name: this.app.t('pins-read-interval') })).first()).toBeVisible();
    return new PinDrawer(this.app, drawer);
  }
}

export class PinDrawer {
  constructor(
    readonly app: AppShell,
    readonly root: Locator,
  ) {}

  async setMode(mode: 'digital_in' | 'digital_out' | 'pwm_out' | 'analog_in'): Promise<void> {
    await this.root.getByRole('combobox', { name: this.app.t('pins-mode') }).selectOption(mode);
    await this.root.getByRole('button', { name: this.app.t('pins-apply-mode'), exact: true }).click();
  }

  async write(level: 'high' | 'low'): Promise<void> {
    await this.root.getByRole('button', { name: this.app.t(`pins-write-${level}`), exact: true }).click();
  }

  /** Periodic read: 0 (manual), 1000, 5000, 15000 or 60000 ms. */
  async subscribe(intervalMs: 0 | 1000 | 5000 | 15000 | 60000): Promise<void> {
    await this.root.getByRole('combobox', { name: this.app.t('pins-read-interval') }).selectOption(String(intervalMs));
    await this.root.getByRole('button', { name: this.app.t('pins-apply'), exact: true }).click();
  }

  async close(): Promise<void> {
    await this.root.getByRole('button', { name: this.app.t('board-drawer-close') }).first().click();
  }
}
