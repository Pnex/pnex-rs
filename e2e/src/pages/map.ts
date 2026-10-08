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
    // The h1 lives in the side panel, closed on phones: wait for the map host.
    await expect(this.page.locator('#pnex-map-host')).toBeVisible();
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

  /** Read-only preview panel of an attached object (covers the map). */
  get preview(): Locator {
    return this.page.locator('[data-testid="poi-preview"]');
  }

  /** Phone: the POI side panel starts closed — open it first. */
  async ensurePanel(): Promise<void> {
    const expand = this.page.getByRole('button', { name: this.app.t('poi-expand') });
    if (await expand.isVisible()) await expand.click();
  }

  /** Phone: the preview covers the drawer — close it to pick another row. */
  async backToDrawer(): Promise<void> {
    const vw = this.page.viewportSize()?.width ?? 1600;
    if (vw < 768) await this.preview.getByRole('button', { name: this.app.t('poi-preview-close') }).click();
  }

  async openPoi(label: string): Promise<void> {
    await this.ensurePanel();
    await this.panel.getByText(label).first().click();
    await expect(this.drawer.getByRole('heading', { level: 2 })).toContainText(label);
  }

  async deleteOpenPoi(): Promise<void> {
    await this.drawer.getByRole('button', { name: this.app.t('viz-delete'), exact: true }).click();
    await confirmDialog(this.page, this.app.t('poi-delete-title')).getByRole('button', { name: this.app.t('viz-delete') }).click();
  }
}
