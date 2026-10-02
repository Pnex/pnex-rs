// Dashboards: list (/dashboards) and the SCADA editor (widget library,
// inspector, save).
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, confirmDialog } from './shell.ts';

export class DashboardsPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  async open(): Promise<void> {
    await this.app.goto('/dashboards');
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-dashboards'));
  }

  row(name: string): Locator {
    return this.page.locator('main tr').filter({ hasText: name });
  }

  /** "New dashboard" creates one immediately and opens it in edit mode. */
  async create(name: string): Promise<DashboardEditor> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('db-create') }).click();
    const editor = new DashboardEditor(this.app);
    await editor.waitReady();
    await editor.rename(name);
    return editor;
  }

  async edit(name: string): Promise<DashboardEditor> {
    await this.row(name).getByRole('button', { name: this.app.t('db-edit') }).click();
    const editor = new DashboardEditor(this.app);
    await editor.waitReady();
    return editor;
  }

  /** From the live view back to the list. */
  async backToList(): Promise<void> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('db-back') }).click();
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-dashboards'));
  }

  /** Live (read-only) view. */
  async view(name: string): Promise<void> {
    await this.row(name).getByRole('button', { name: this.app.t('db-open') }).click();
  }

  async delete(name: string): Promise<void> {
    await this.row(name).getByRole('button', { name: this.app.t('viz-delete') }).click();
    const dlg = confirmDialog(this.page, this.app.t('db-delete'));
    await dlg.getByRole('button', { name: this.app.t('common-delete') }).click();
    await expect(this.row(name)).toHaveCount(0);
  }
}

export class DashboardEditor {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  get main(): Locator {
    return this.page.getByRole('main');
  }

  /** Widget inspector (right panel). */
  get inspector(): Locator {
    return this.main.getByRole('complementary').last();
  }

  async waitReady(): Promise<void> {
    await expect(this.main.getByRole('button', { name: this.app.t('eshell-add-widget') })).toBeVisible();
  }

  async rename(name: string): Promise<void> {
    await this.main.getByRole('button').nth(1).click();
    const input = this.main.locator('input:not([placeholder])').first();
    await input.fill(name);
    await input.press('Enter');
    await expect(this.main.getByRole('button', { name })).toBeVisible();
  }

  /** Adds a widget from the library by its exact label. */
  async addWidget(label: string): Promise<void> {
    const search = this.main.getByRole('textbox', { name: this.app.t('eshell-search') });
    if (!(await search.isVisible())) {
      await this.main.getByRole('button', { name: this.app.t('eshell-add-widget') }).click();
    }
    await this.main.getByRole('button', { name: label, exact: true }).click();
    await expect(this.inspector).toBeVisible();
  }

  field(key: string): Locator {
    return this.inspector.getByLabel(this.app.t(key), { exact: true });
  }

  get saveButton(): Locator {
    return this.main.getByRole('button', { name: this.app.t('db-save'), exact: true });
  }

  /** Save and wait for the server's confirmation (the button is also disabled while saving). */
  async save(): Promise<void> {
    await this.saveButton.click();
    await this.app.expectToast(this.app.tr('db-saved'));
    await expect(this.saveButton).toBeDisabled();
  }

  /** Leaves edit mode: lands on the live view of the same dashboard. */
  async back(): Promise<void> {
    await this.main.getByRole('button', { name: this.app.t('eshell-back') }).click();
    await expect(this.main.getByRole('button', { name: this.app.t('db-back') })).toBeVisible();
  }
}
