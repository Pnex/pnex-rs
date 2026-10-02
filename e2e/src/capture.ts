// Annotated screenshots for tests that document a feature.
//
// `capture()` is a no-op unless PNEX_E2E_CAPTURE_DIR is set. When it is,
// every call writes `<dir>/<locale>/<test-slug>/<NN>-<name>.png` and appends
// an entry to `<dir>/manifest.jsonl` (one JSON object per line), so a
// downstream tool can rebuild the ordered story of each test.
import { appendFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import type { Locator, Page, TestInfo } from '@playwright/test';
import { CAPTURE_DIR } from './env.ts';

export interface CaptureOptions {
  /** One-line description of what the shot shows. */
  caption?: string;
  /** Element to crop to (whole viewport otherwise). */
  target?: Locator;
  /** Full scrollable page instead of the viewport. */
  fullPage?: boolean;
  /** Extra selectors to blur on this shot only. */
  mask?: Locator[];
}

export interface CaptureEntry {
  test: string;
  titlePath: string[];
  file: string;
  locale: string;
  index: number;
  name: string;
  caption?: string;
  url: string;
  tags: string[];
  at: string;
}

/** Hides what never belongs in a screenshot: secrets and transient UI. */
export const CAPTURE_CSS = `
  [data-doc-mask], .doc-mask { filter: blur(6px) !important; }
  button[aria-label="AI Assistant"] { display: none !important; }
  .fixed.top-4.right-4.z-50 { visibility: hidden !important; }
`;

export const captureEnabled = (): boolean => !!CAPTURE_DIR;

function slug(s: string): string {
  return s
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^\w]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80);
}

export class Capturer {
  private index = 0;

  constructor(
    private readonly page: Page,
    private readonly info: TestInfo,
    private readonly locale: string,
  ) {}

  async capture(name: string, opts: CaptureOptions = {}): Promise<string | undefined> {
    if (!CAPTURE_DIR) return undefined;
    this.index += 1;
    const testSlug = slug(this.info.titlePath.slice(1).join(' '));
    const rel = path.join(this.locale, testSlug, `${String(this.index).padStart(2, '0')}-${slug(name)}.png`);
    const file = path.join(CAPTURE_DIR, rel);
    mkdirSync(path.dirname(file), { recursive: true });

    // Lets layout, fonts and fade-ins settle.
    await this.page.waitForTimeout(400);
    // `style` applies only while shooting: toasts stay assertable afterwards.
    const shotOpts = { path: file, mask: opts.mask, animations: 'disabled' as const, style: CAPTURE_CSS };
    if (opts.target) await opts.target.first().screenshot(shotOpts);
    else await this.page.screenshot({ ...shotOpts, fullPage: opts.fullPage });

    const entry: CaptureEntry = {
      test: this.info.title,
      titlePath: this.info.titlePath.slice(1),
      file: rel,
      locale: this.locale,
      index: this.index,
      name,
      caption: opts.caption,
      url: new URL(this.page.url()).pathname,
      tags: this.info.tags,
      at: new Date().toISOString(),
    };
    appendFileSync(path.join(CAPTURE_DIR, 'manifest.jsonl'), JSON.stringify(entry) + '\n');
    return file;
  }
}

/** Blurs every leaf element whose text contains one of `secrets`. */
export async function maskText(page: Page, secrets: string[]): Promise<void> {
  await page.evaluate((list) => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
    for (let n = walker.nextNode() as HTMLElement | null; n; n = walker.nextNode() as HTMLElement | null) {
      if (n.children.length) continue;
      const txt = `${n.textContent ?? ''} ${(n as HTMLInputElement).value ?? ''}`;
      if (list.some((s) => s && txt.includes(s))) n.classList.add('doc-mask');
    }
  }, secrets);
}
