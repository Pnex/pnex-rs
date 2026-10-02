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

/** Equirectangular-looking 2:1 PNG (sky, horizon, floor) for 360° scenes. */
export async function makePanorama(browser: Browser, label: string, w = 2048): Promise<Buffer> {
  const h = w / 2;
  const page = await browser.newPage({ viewport: { width: w, height: h } });
  await page.setContent(
    `<body style="margin:0;width:${w}px;height:${h}px;background:linear-gradient(#7dd3fc 0%,#e0f2fe 48%,#a3a3a3 52%,#525252 100%);` +
      `display:flex;align-items:center;justify-content:space-around;font:bold 64px sans-serif;color:#0f172a">` +
      `<span>N</span><span>${label}</span><span>S</span><span>W</span></body>`,
  );
  const png = await page.screenshot({ type: 'png' });
  await page.close();
  return png;
}

/** Simple floor plan: walls and labelled rooms. */
export async function makeFloorPlan(browser: Browser, rooms: string[], w = 1200, h = 800): Promise<Buffer> {
  const page = await browser.newPage({ viewport: { width: w, height: h } });
  const cells = rooms
    .map((r) => `<div style="border:6px solid #334155;display:flex;align-items:center;justify-content:center;font:bold 36px sans-serif;color:#334155">${r}</div>`)
    .join('');
  await page.setContent(
    `<body style="margin:0;width:${w}px;height:${h}px;background:#f8fafc;display:grid;grid-template-columns:repeat(${Math.min(rooms.length, 3)},1fr);padding:24px;box-sizing:border-box;gap:0">${cells}</body>`,
  );
  const png = await page.screenshot({ type: 'png' });
  await page.close();
  return png;
}
