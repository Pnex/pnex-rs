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

  /**
   * "New dashboard" opens the creation modal (D123): name, format card,
   * "Create and open" — the editor opens on the new dashboard.
   */
  async create(name: string, format: 'desktop' | 'mobile' = 'desktop'): Promise<DashboardEditor> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('db-create') }).click();
    const dlg = this.page.getByRole('dialog', { name: this.app.t('db-new-title') });
    await dlg.getByLabel(this.app.t('db-new-name'), { exact: true }).fill(name);
    const title = this.app.t(format === 'mobile' ? 'db-format-mobile' : 'db-format-desktop');
    await dlg
      .getByRole('button')
      .filter({ has: this.page.getByText(title, { exact: true }) })
      .click();
    await dlg.getByRole('button', { name: this.app.t('db-new-create') }).click();
    const editor = new DashboardEditor(this.app);
    await editor.waitReady();
    await expect(this.page.getByRole('main').getByRole('button', { name })).toBeVisible();
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
    // Title button (tooltip "Rename"), not a toolbar position.
    await this.main.getByTitle(this.app.t('eshell-rename')).first().click();
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
    // Items with a description carry it in their accessible name.
    const escaped = label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    await this.main.getByRole('button', { name: new RegExp(`^${escaped}(\\s|$)`) }).first().click();
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
    await this.main.getByRole('button', { name: this.app.t('eshell-back'), exact: true }).click();
    await expect(this.main.getByRole('button', { name: this.app.t('db-back') })).toBeVisible();
  }
}
