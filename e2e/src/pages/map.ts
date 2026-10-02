// Map page (/map): POI side panel, add mode, POI drawer.
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, confirmDialog, dialog, fieldAfterLabel } from './shell.ts';

export class MapPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  async open(): Promise<void> {
    await this.app.goto('/map');
    await expect(this.page.getByRole('heading', { name: this.app.t('viz-map-title'), level: 1 })).toBeVisible();
    // Tiles and the maplibre canvas settle after mount.
    await this.page.waitForTimeout(1500);
  }

  get map(): Locator {
    return this.page.getByRole('region', { name: this.app.t('viz-map-title') });
  }

  /** Left panel (the POI drawer is the other complementary, right side). */
  get panel(): Locator {
    return this.page.getByRole('main').getByRole('complementary').first();
  }

  get drawer(): Locator {
    return this.page.locator('aside.right-0');
  }

  /** Add mode, click at the map centre (+ offset), fill the dialog. */
  async addPoi(label: string, pictogram?: string, offset = { x: 0, y: 0 }): Promise<void> {
    await this.page.getByRole('button', { name: this.app.t('poi-add') }).click();
    const box = (await this.map.boundingBox())!;
    await this.page.mouse.click(box.x + box.width / 2 + offset.x, box.y + box.height / 2 + offset.y);
    const dlg = dialog(this.page, this.app.t('poi-add-title'));
    await fieldAfterLabel(dlg, this.app.t('poi-field-label')).fill(label);
    if (pictogram) await dlg.getByRole('button', { name: pictogram, exact: true }).click();
    await dlg.getByRole('button', { name: this.app.t('poi-save'), exact: true }).click();
    await expect(dlg).toBeHidden();
  }

  async openPoi(label: string): Promise<void> {
    await this.panel.getByText(label).first().click();
    await expect(this.drawer.getByRole('heading', { level: 2 })).toContainText(label);
  }

  async deleteOpenPoi(): Promise<void> {
    await this.drawer.getByRole('button', { name: this.app.t('viz-delete'), exact: true }).click();
    await confirmDialog(this.page, this.app.t('poi-delete-title')).getByRole('button', { name: this.app.t('viz-delete') }).click();
  }
}
