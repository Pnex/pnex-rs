// Functions page (/functions): list, creation dialog, code editor, live test.
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, confirmDialog, dialog } from './shell.ts';

export type FunctionLanguage = 'js' | 'starlark';
export type FunctionTemplate = 'empty' | 'threshold' | 'unit' | 'payload';

export class FunctionsPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  private t(key: string): string {
    return this.app.t(key);
  }

  async open(): Promise<void> {
    await this.app.goto('/functions');
    await expect(this.page.locator('main h1')).toHaveText(this.t('nav-functions'));
  }

  row(name: string): Locator {
    return this.page.locator('main tr').filter({ hasText: name });
  }

  /** Creates a function from the dialog; the editor opens on it. */
  async create(name: string, language: FunctionLanguage, template: FunctionTemplate = 'empty'): Promise<FunctionEditor> {
    await this.page.getByRole('main').getByRole('button', { name: this.t('functions-new') }).first().click();
    const dlg = dialog(this.page, this.t('functions-create-title'));
    await dlg.getByRole('textbox', { name: this.t('functions-field-name') }).fill(name);
    await dlg.getByRole('radio', { name: new RegExp(`^${this.t(`functions-lang-${language}`)}`) }).check();
    await dlg.getByRole('radio', { name: new RegExp(`^${this.t(`functions-tpl-${template}`)}`) }).check();
    await dlg.getByRole('button', { name: this.t('functions-new'), exact: true }).click();
    await expect(dlg).toBeHidden();
    const editor = new FunctionEditor(this.app);
    await editor.waitReady(name);
    return editor;
  }

  async openFunction(name: string): Promise<FunctionEditor> {
    await this.row(name).getByRole('button', { name: this.t('functions-open') }).click();
    const editor = new FunctionEditor(this.app);
    await editor.waitReady(name);
    return editor;
  }

  async delete(name: string): Promise<void> {
    await this.row(name).getByRole('button', { name: this.t('functions-delete') }).click();
    const dlg = confirmDialog(this.page, this.t('functions-confirm-delete-title'));
    await dlg.getByRole('button', { name: this.t('common-delete') }).click();
    await expect(this.row(name)).toHaveCount(0);
  }
}

export interface TestRunResult {
  ok: boolean;
  /**
   * Output name (port label before " · port N") → emitted value, parsed as
   * JSON when possible (muted ports omitted).
   */
  outputs: Record<string, unknown>;
  error?: string;
}

export class FunctionEditor {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  private t(key: string): string {
    return this.app.t(key);
  }

  async waitReady(name: string): Promise<void> {
    await expect(this.nameInput).toHaveValue(name);
  }

  get nameInput(): Locator {
    return this.page.locator('main input[type=text], main input:not([type])').first();
  }

  /** Code area (textarea under the syntax overlay). */
  get code(): Locator {
    return this.page.locator('main textarea').first();
  }

  get saveButton(): Locator {
    return this.page.getByRole('button', { name: new RegExp(`^${this.t('functions-save')}`) });
  }

  /** Version badge text ("Starlark v2"). */
  version(): Locator {
    return this.page.getByRole('main').getByText(/\bv\d+\b/).first();
  }

  async setCode(code: string): Promise<void> {
    await this.code.fill(code);
  }

  /** Clicks the "declare all" quick-fix under the code. */
  async declareAll(): Promise<void> {
    await this.page.getByRole('button', { name: this.app.tr('functions-fix-all') }).first().click();
  }

  /** Save and wait for the server's confirmation (the button is also disabled while saving). */
  async save(): Promise<void> {
    await this.saveButton.click();
    await this.app.expectToast(this.app.t('toast-function-saved'));
    await expect(this.saveButton).toBeDisabled();
  }

  /** Opens the live test, fills declared inputs by label, runs it. */
  async testRun(inputs: Record<string, string | number> = {}, msg?: object): Promise<TestRunResult> {
    await this.page.getByRole('button', { name: this.t('functions-test-open') }).click();
    const dlg = dialog(this.page, this.t('functions-test-title'));
    for (const [name, value] of Object.entries(inputs)) {
      await dlg.getByLabel(new RegExp(`^${name} \\(`)).fill(String(value));
    }
    if (msg) await dlg.getByRole('textbox', { name: this.t('functions-test-msg-field') }).fill(JSON.stringify(msg));
    await dlg.getByRole('button', { name: this.t('functions-test-execute'), exact: true }).click();

    const okBlock = dlg.getByText(this.t('functions-test-outputs'));
    const errBlock = dlg.getByText(this.t('functions-test-error'));
    await expect(okBlock.or(errBlock)).toBeVisible();
    if (await errBlock.isVisible()) {
      const error = await errBlock.locator('xpath=following-sibling::p[1]').textContent();
      return { ok: false, outputs: {}, error: error ?? '' };
    }
    const outputs: Record<string, unknown> = {};
    for (const row of await dlg.locator('div.bg-gray-900:has(> span.font-mono)').all()) {
      const label = ((await row.locator('span').first().textContent()) ?? '').split(' · ')[0].trim();
      const pre = row.locator('pre');
      if (!(await pre.count())) continue;
      const text = ((await pre.textContent()) ?? '').trim();
      try {
        outputs[label] = JSON.parse(text);
      } catch {
        outputs[label] = text;
      }
    }
    return { ok: true, outputs };
  }

  async closeTest(): Promise<void> {
    await dialog(this.page, this.t('functions-test-title')).getByRole('button', { name: this.t('common-cancel') }).click();
  }

  /** Back to the list (editor back arrow). */
  async back(): Promise<void> {
    await this.app.goto('/functions');
  }
}
