// Notifications page (/notifications): channels (kind picker + generated
// form), templates.
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, confirmDialog, dialog } from './shell.ts';

export class NotificationsPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  async open(): Promise<void> {
    await this.app.goto('/notifications');
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-notifications'));
  }

  /** Innermost block of the channel list holding `name` and its actions. */
  channel(name: string): Locator {
    return this.page
      .getByRole('main')
      .locator('div')
      .filter({ has: this.page.getByText(name, { exact: true }) })
      .filter({ has: this.page.getByRole('button', { name: this.app.t('notify-delete'), exact: true }) })
      .last();
  }

  /** Opens the channel form for a kind (`smtp`, `webhook`, `ntfy`…). */
  async newChannel(kind: string): Promise<ChannelForm> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('notify-new-channel') }).click();
    const picker = dialog(this.page, this.app.t('notify-pick-title'));
    await picker.getByRole('button', { name: new RegExp(`^${escapeRe(this.app.t(`notify-kind-${kind}`))}`) }).click();
    const form = new ChannelForm(this.app, dialog(this.page, this.app.t('notify-new-channel-title')));
    await expect(form.root).toBeVisible();
    return form;
  }

  async deleteChannel(name: string): Promise<void> {
    await this.channel(name).getByRole('button', { name: this.app.t('notify-delete'), exact: true }).click();
    const dlg = confirmDialog(this.page, this.app.t('notify-confirm-delete-title'));
    await dlg.getByRole('button', { name: this.app.t('notify-delete'), exact: true }).click();
    await expect(this.page.getByRole('main').getByText(name, { exact: true })).toHaveCount(0);
  }
}

export class ChannelForm {
  constructor(
    readonly app: AppShell,
    readonly root: Locator,
  ) {}

  /** Field by its ftl label key (required marker tolerated). */
  field(key: string): Locator {
    return this.root.getByLabel(new RegExp(`^${escapeRe(this.app.t(key))}\\s*\\*?$`));
  }

  async name(value: string): Promise<void> {
    await this.field('notify-field-name').fill(value);
  }

  /** Runs the form's "Test" (draft config); resolves when it is back. */
  async test(): Promise<void> {
    await this.root.getByRole('button', { name: this.app.t('notify-test'), exact: true }).click();
    await expect(this.root.getByRole('button', { name: this.app.t('notify-test'), exact: true })).toBeEnabled({ timeout: 30_000 });
  }

  async create(): Promise<void> {
    await this.root.getByRole('button', { name: this.app.t('common-create'), exact: true }).click();
    await expect(this.root).toBeHidden();
  }
}

export function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
