// Phone layout probe (docs/architecture/mobile-ui.md): on the current page,
// lists what a phone user cannot reach — a page that scrolls sideways, an
// action (button, link, select) past the right edge of the screen that no
// scrollable ancestor brings back, a row action of a table outside the
// viewport (the actions column must stick to the right edge).
import type { Page } from 'playwright';

export interface LayoutIssue {
  kind: 'page-overflow' | 'action-offscreen' | 'row-action-offscreen';
  /** Short description of the element (tag, text or label, right edge). */
  what: string;
}

export async function mobileLayoutIssues(page: Page): Promise<LayoutIssue[]> {
  return page.evaluate(() => {
    const issues: { kind: string; what: string }[] = [];
    const vw = window.innerWidth;
    // 1 px of slack: sub-pixel rounding of borders.
    const SLACK = 1;
    const doc = document.documentElement;
    if (doc.scrollWidth > vw + SLACK) {
      issues.push({ kind: 'page-overflow', what: `scrollWidth ${doc.scrollWidth} > ${vw}` });
    }

    const visible = (el: Element) => {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return false;
      const s = getComputedStyle(el);
      return s.visibility !== 'hidden' && s.display !== 'none';
    };
    const describe = (el: Element) => {
      const label =
        el.getAttribute('aria-label') ||
        el.getAttribute('title') ||
        (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
      return `${el.tagName.toLowerCase()} "${label}" right=${Math.round(el.getBoundingClientRect().right)}`;
    };
    /** Inside a container the user can scroll sideways (table, pinout…)? */
    const scrollable = (el: Element) => {
      for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
        const ox = getComputedStyle(p).overflowX;
        if ((ox === 'auto' || ox === 'scroll') && p.scrollWidth > p.clientWidth) return true;
      }
      return false;
    };

    const main = document.querySelector('main') ?? document.body;
    for (const el of main.querySelectorAll('button, a[href], select')) {
      if (!visible(el)) continue;
      // Row actions are checked below: they must stay on screen even inside
      // a scrollable table.
      if (el.closest('td')) continue;
      if (el.getBoundingClientRect().right > vw + SLACK && !scrollable(el)) {
        issues.push({ kind: 'action-offscreen', what: describe(el) });
      }
    }
    for (const el of main.querySelectorAll('tr > td:last-child button, tr > td:last-child a[href]')) {
      if (!visible(el)) continue;
      if (el.getBoundingClientRect().right > vw + SLACK) {
        issues.push({ kind: 'row-action-offscreen', what: describe(el) });
      }
    }
    return issues;
  }) as Promise<LayoutIssue[]>;
}
