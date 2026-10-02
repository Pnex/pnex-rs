// Run configuration, resolved once from the environment.
//
// Defaults target the local stack: the TLS edge when `deploy/edge/edge.env`
// exists (`https://<PNEX_DOMAIN>`), the plain dev server otherwise.
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

function edgeDomain(): string | undefined {
  const file = path.join(REPO_ROOT, 'deploy/edge/edge.env');
  if (!existsSync(file)) return undefined;
  const m = readFileSync(file, 'utf8').match(/^PNEX_DOMAIN=(.+)$/m);
  return m?.[1].trim();
}

function trimSlash(url: string): string {
  return url.replace(/\/+$/, '');
}

const domain = edgeDomain();

/** Origin serving the web UI and `/api/v1`. */
export const BASE_URL = trimSlash(
  process.env.PNEX_E2E_BASE_URL ?? (domain ? `https://${domain}` : 'http://localhost:5150'),
);

/** Rauthy issuer (trailing slash), used for the password grant. */
export const ISSUER =
  process.env.PNEX_E2E_ISSUER ??
  (domain || !BASE_URL.endsWith(':5150') ? `${BASE_URL}/auth/v1/` : 'http://localhost:8080/auth/v1/');

export const EMAIL = process.env.PNEX_E2E_EMAIL ?? 'admin@example.com';
export const PASSWORD = process.env.PNEX_E2E_PASSWORD ?? 'admin-pass';
export const CLIENT_ID = process.env.PNEX_E2E_CLIENT_ID ?? 'pnex';

/**
 * Organization the tests run in. Created on first run, never the operator's
 * own org: tests may create and delete anything inside it.
 */
export const ORG_NAME = process.env.PNEX_E2E_ORG ?? 'E2E';

/** `1` drops and recreates the test org before the run (clean slate). */
export const FRESH_ORG = process.env.PNEX_E2E_FRESH_ORG === '1';

/** Prefix of every resource a test creates (cleanup and filtering key). */
export const RUN_ID = process.env.PNEX_E2E_RUN_ID ?? new Date().toISOString().slice(5, 16).replace(/[-:T]/g, '');
export const PREFIX = `e2e-${RUN_ID}`;

/** When set, `capture()` writes annotated screenshots + a manifest there. */
export const CAPTURE_DIR = process.env.PNEX_E2E_CAPTURE_DIR;

/** Name of a live device for tests tagged `@hardware` (skipped when unset). */
export const DEVICE = process.env.PNEX_E2E_DEVICE;

/** Where the global setup leaves the shared state (tokens, org id). */
export const STATE_FILE = path.join(REPO_ROOT, 'e2e/.state/run.json');
