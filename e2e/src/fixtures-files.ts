// Synthetic files for upload tests (no binary fixtures in the repo).
import type { Browser } from '@playwright/test';

/** A PNG rendered by the browser: a labelled gradient, `w`×`h` pixels. */
export async function makePng(browser: Browser, label: string, w = 640, h = 400): Promise<Buffer> {
  const page = await browser.newPage({ viewport: { width: w, height: h } });
  await page.setContent(
    `<body style="margin:0;width:${w}px;height:${h}px;background:linear-gradient(135deg,#0ea5e9,#22c55e);` +
      `display:flex;align-items:center;justify-content:center;font:bold 36px sans-serif;color:white">${label}</body>`,
  );
  const png = await page.screenshot({ type: 'png' });
  await page.close();
  return png;
}
