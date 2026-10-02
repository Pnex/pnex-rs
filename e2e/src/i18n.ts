// Reads the front's Fluent catalogs so assertions follow the real labels in
// both locales instead of hard-coding UI strings.
//
// Single-line messages only (`key = value`); placeables (`{ $x }`) become
// wildcards in `tRe`. Selectors and multi-line messages are not supported.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { REPO_ROOT } from './env.ts';

const catalogs = new Map<string, Map<string, string>>();

function catalog(locale: string): Map<string, string> {
  const tag = locale.startsWith('fr') ? 'fr-FR' : 'en-US';
  let cat = catalogs.get(tag);
  if (!cat) {
    cat = new Map();
    const file = path.join(REPO_ROOT, `crates/pnex-frontend/locales/${tag}.ftl`);
    for (const line of readFileSync(file, 'utf8').split('\n')) {
      const m = line.match(/^([a-zA-Z][\w-]*)\s*=\s*(.+)$/);
      if (m) cat.set(m[1], m[2].trim());
    }
    catalogs.set(tag, cat);
  }
  return cat;
}

/** Raw message of `key` (placeables left as written). Throws if absent. */
export function t(locale: string, key: string): string {
  const v = catalog(locale).get(key);
  if (v == null) throw new Error(`ftl key not found (${locale}): ${key}`);
  // Literal braces are written {"{"} in ftl.
  return v.replace(/\{\s*"([^"]*)"\s*\}/g, '$1');
}

function escape(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Exact-match RegExp for `key`: placeables match anything, and the bidi
 * isolation marks Fluent wraps them in (U+2068/U+2069) are tolerated.
 */
export function tRe(locale: string, key: string, flags = ''): RegExp {
  const parts = t(locale, key).split(/\{\s*\$[\w-]+\s*\}/);
  return new RegExp(`^\\s*${parts.map(escape).join('[\\u2068\\u2069]?.*?[\\u2068\\u2069]?')}\\s*$`, flags);
}
