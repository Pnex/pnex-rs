// Pages (/pages): list, creation dialog, live editor (ProseMirror + Yjs),
// versions and comments panels.
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, dialog } from './shell.ts';

export class DocsPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  private t(key: string): string {
    return this.app.t(key);
  }

  async open(): Promise<void> {
    await this.app.goto('/pages');
    await expect(this.page.locator('main h1')).toHaveText(this.t('docs-title'));
  }

  row(title: string): Locator {
    return this.page.locator('main tr').filter({ hasText: title });
  }

  /** Creates a page from the dialog; the editor opens on it. */
  async create(title: string): Promise<DocEditor> {
    await this.page.getByRole('main').getByRole('button', { name: this.t('docs-new') }).first().click();
    const dlg = dialog(this.page, this.t('docs-new-title'));
    await dlg.getByRole('textbox', { name: this.t('docs-field-title') }).fill(title);
    await dlg.getByRole('button', { name: this.t('docs-create'), exact: true }).click();
    await expect(dlg).toBeHidden();
    const editor = new DocEditor(this.app, this.page);
    await editor.waitSynced();
    return editor;
  }

  async delete(title: string): Promise<void> {
    await this.row(title).getByRole('button', { name: this.t('docs-delete') }).click();
    await this.row(title).getByRole('button', { name: this.t('docs-confirm-delete') }).click();
    await expect(this.row(title)).toHaveCount(0);
  }
}

export class DocEditor {
  constructor(
    readonly app: AppShell,
    readonly page: Page,
  ) {}

  private t(key: string): string {
    return this.app.t(key);
  }

  /** The live editor (not the read-only version preview). */
  get content(): Locator {
    return this.page.locator('[id^="pnex-doc-"]:not(#pnex-doc-version) .ProseMirror');
  }

  get id(): string {
    return new URL(this.page.url()).searchParams.get('id') ?? '';
  }

  async waitSynced(): Promise<void> {
    await expect(this.content).toBeVisible();
    await expect(this.page.getByText(this.t('docs-status-synced'), { exact: true })).toBeVisible({ timeout: 15_000 });
  }

  /** Types at the end of the page. */
  async typeAtEnd(text: string): Promise<void> {
    // Click the last block, not the editor: in a freshly opened tab the
    // shortcut alone sometimes fired before focus and typed at the start.
    await this.content.locator(':scope > *').last().click();
    await this.page.keyboard.press('ControlOrMeta+End');
    await this.page.keyboard.type(text);
  }

  /** Selects the whole last block (for marks, links and comments). */
  async selectLastLine(): Promise<void> {
    await this.content.click();
    await this.page.keyboard.press('ControlOrMeta+End');
    await this.page.keyboard.press('Shift+Home');
  }

  async toolbar(key: string): Promise<void> {
    await this.page.getByRole('button', { name: this.t(key), exact: true }).first().click();
  }

  async openVersions(): Promise<Locator> {
    await this.page.getByRole('button', { name: this.t('docs-versions') }).first().click();
    return this.page.locator('aside').filter({ has: this.page.getByRole('heading', { name: this.t('docs-versions') }) });
  }

  async openComments(): Promise<Locator> {
    const panel = this.commentsPanel;
    if (!(await panel.isVisible())) {
      await this.page.getByRole('button', { name: this.t('docs-comments') }).first().click();
    }
    return panel;
  }

  get commentsPanel(): Locator {
    return this.page.locator('aside').filter({ has: this.page.getByRole('heading', { name: this.t('docs-comments') }) });
  }
}
