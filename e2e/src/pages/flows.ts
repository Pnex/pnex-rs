// Flows: list (/flows) and the SVG flow editor (palette, inspector, wiring,
// save / deploy / stop).
import { expect, type Locator, type Page } from '@playwright/test';
import { AppShell, confirmDialog } from './shell.ts';

export interface Port {
  x: number;
  y: number;
  label: string;
}

export interface NodeBox {
  title: string;
  x: number;
  y: number;
  w: number;
  h: number;
  outs: Port[];
  ins: Port[];
}

export class FlowsPage {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  async open(): Promise<void> {
    await this.app.goto('/flows');
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-flows'));
  }

  row(name: string): Locator {
    return this.page.locator('main tr').filter({ hasText: name });
  }

  /** "New flow" creates a flow immediately and opens the editor on it. */
  async create(name: string): Promise<FlowEditor> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('flows-new') }).click();
    const editor = new FlowEditor(this.app);
    await editor.waitReady();
    await editor.rename(name);
    return editor;
  }

  async openFlow(name: string): Promise<FlowEditor> {
    await this.row(name).getByRole('button', { name: this.app.tr('flows-open', 'i') }).click();
    const editor = new FlowEditor(this.app);
    await editor.waitReady();
    return editor;
  }

  async delete(name: string): Promise<void> {
    await this.row(name).getByRole('button', { name: this.app.t('flows-delete') }).click();
    const dlg = confirmDialog(this.page, this.app.t('flows-confirm-delete-title'));
    await dlg.getByRole('button', { name: this.app.t('flows-delete') }).click();
    await expect(this.row(name)).toHaveCount(0);
  }
}

export class FlowEditor {
  readonly page: Page;

  constructor(readonly app: AppShell) {
    this.page = app.page;
  }

  get canvas(): Locator {
    return this.page.locator('#flow-canvas');
  }

  /** Right-hand node inspector. */
  get inspector(): Locator {
    return this.page.getByRole('main').getByRole('complementary').last();
  }

  async waitReady(): Promise<void> {
    await this.canvas.waitFor();
    // The first layout pass positions nodes after mount.
    await this.page.waitForTimeout(800);
  }

  /** Title button of the editor shell (click → inline rename). */
  get titleButton(): Locator {
    return this.page.getByRole('main').getByRole('button').nth(1);
  }

  async rename(name: string): Promise<void> {
    await this.titleButton.click();
    const input = this.page.getByRole('main').locator('input:not([placeholder])').first();
    await input.fill(name);
    await input.press('Enter');
    await expect(this.page.getByRole('main').getByRole('button', { name })).toBeVisible();
  }

  /** Picks a palette entry (by its ftl key); the node lands selected. */
  async addNode(paletteKey: string): Promise<void> {
    const label = this.app.t(paletteKey);
    const search = this.page.getByRole('main').getByRole('textbox', { name: this.app.t('eshell-search') });
    if (!(await search.isVisible())) {
      await this.page.getByRole('button', { name: this.app.t('eshell-add-node') }).click();
    }
    await search.fill(label);
    await this.page.getByRole('button', { name: new RegExp(`^${label}\\b`) }).first().click();
    await expect(this.inspector).toContainText(label);
  }

  /** Screen-space boxes of every node group (DOM order). */
  async nodes(): Promise<NodeBox[]> {
    return this.page.evaluate(() => {
      const out: NodeBox[] = [];
      for (const g of document.querySelectorAll('#flow-canvas g[transform^="translate"]')) {
        const rect = g.querySelector(':scope > rect');
        const title = g.querySelector(':scope > text');
        if (!rect || !title) continue;
        const b = rect.getBoundingClientRect();
        const center = (c: Element) => {
          const r = c.getBoundingClientRect();
          return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
        };
        const outs = [...g.querySelectorAll('circle[r="7"]')].map((c) => ({
          ...center(c),
          label: c.parentElement?.querySelector('text')?.textContent ?? '',
        }));
        const ins = [...g.querySelectorAll('circle[r="10"]')].map((c) => ({
          ...center(c),
          label:
            c.parentElement && c.parentElement.tagName === 'g' && c.parentElement !== g
              ? c.parentElement.querySelector('text')?.textContent ?? ''
              : '',
        }));
        out.push({ title: (title.textContent ?? '').trim(), x: b.x, y: b.y, w: b.width, h: b.height, outs, ins });
      }
      return out;
    });
  }

  async node(title: string, nth = 0): Promise<NodeBox> {
    const all = (await this.nodes()).filter((n) => n.title === title);
    if (!all[nth]) throw new Error(`node not found: ${title} #${nth} in ${JSON.stringify((await this.nodes()).map((n) => n.title))}`);
    return all[nth];
  }

  async select(title: string, nth = 0): Promise<void> {
    const n = await this.node(title, nth);
    await this.page.mouse.click(n.x + n.w / 2, n.y + n.h / 2);
    await expect(this.inspector).toBeVisible();
  }

  /** Drags a node so its top-left corner lands at canvas coordinates (x, y). */
  async move(title: string, x: number, y: number, nth = 0): Promise<void> {
    const n = await this.node(title, nth);
    const box = (await this.canvas.boundingBox())!;
    const grabX = n.x + n.w * 0.3;
    const grabY = n.y + 6;
    await this.page.mouse.move(grabX, grabY);
    await this.page.mouse.down();
    await this.page.mouse.move(box.x + x + n.w * 0.3, box.y + y + 6, { steps: 12 });
    await this.page.mouse.up();
  }

  /** Wires output `port` (label or index) of `from` to input `anchor` of `to`. */
  async wire(from: string, port: string | number, to: string, anchor?: string): Promise<void> {
    const src = await this.node(from);
    const dst = await this.node(to);
    const out = typeof port === 'number' ? src.outs[port] : src.outs.find((o) => o.label === port);
    const inp = anchor == null ? dst.ins[0] : dst.ins.find((i) => i.label === anchor);
    if (!out) throw new Error(`output ${port} not found on ${from}: ${JSON.stringify(src.outs)}`);
    if (!inp) throw new Error(`input ${anchor} not found on ${to}: ${JSON.stringify(dst.ins)}`);
    await this.page.mouse.move(out.x, out.y);
    await this.page.mouse.down();
    await this.page.mouse.move((out.x + inp.x) / 2, (out.y + inp.y) / 2, { steps: 6 });
    await this.page.mouse.move(inp.x, inp.y, { steps: 6 });
    await this.page.mouse.up();
  }

  /** Inspector field by its visible label. */
  field(label: string | RegExp): Locator {
    return this.inspector.getByLabel(label).first();
  }

  async closeInspector(): Promise<void> {
    await this.inspector.getByRole('button', { name: this.app.t('eshell-close') }).click();
  }

  get saveButton(): Locator {
    return this.page.getByRole('button', { name: this.app.t('common-save') });
  }

  get deployButton(): Locator {
    return this.page.getByRole('main').getByRole('button', { name: this.app.t('flows-deploy'), exact: true });
  }

  async save(): Promise<void> {
    await this.saveButton.click();
    await expect(this.saveButton).toBeDisabled();
  }

  async deploy(): Promise<void> {
    await this.deployButton.click();
    await this.app.expectToast(this.app.t('toast-flow-deployed'));
  }

  async stop(): Promise<void> {
    await this.page.getByRole('button', { name: this.app.t('flows-stop'), exact: true }).click();
    await this.app.expectToast(this.app.t('toast-flow-stopped'));
  }

  async back(): Promise<void> {
    await this.page.getByRole('main').getByRole('button', { name: this.app.t('eshell-back') }).click();
    await expect(this.page.locator('main h1')).toHaveText(this.app.t('nav-flows'));
  }
}
