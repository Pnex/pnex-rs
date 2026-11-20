// MetaTrader 5 terminals (markets-research.md §3.9, D847–D849): a
// terminal is registered from the list and opens on its connection steps;
// the list shows its presence. Worst case (break-ui): served rows with very
// long names and brokers, every presence, a full page.
import { expect, test } from '../src/fixtures.ts';

const terminal = (i: number, name: string, presence: string) => ({
  id: `00000000-0000-4000-8000-${String(i).padStart(12, '0')}`,
  name,
  broker: i % 2 ? 'Broker with an extraordinarily long legal name that never wraps nicely Ltd' : '',
  server: i % 3 ? `Live-Server-${i}` : '',
  account: String(100000 + i),
  bars: i % 2 ? '*' : 'EURUSD, XAUUSD, US500.cash, GER40.cash, JP225.cash, USOIL.cash',
  ticks: i % 4 ? '' : 'XAUUSD',
  has_token: i % 2 === 0,
  presence,
  symbols: i * 37,
  backfill_days: 0,
  version: 1,
  created_at: '2026-11-01T08:00:00Z',
  updated_at: '2026-11-01T08:00:00Z',
});

test.describe('mt5 terminals', { tag: ['@markets', '@i18n'] }, () => {
  test('a terminal is registered and opens on its connection steps', async ({ app, page, prefix }) => {
    await app.goto('/mt5-terminals');
    await expect(page.getByRole('heading', { name: app.t('mt5-title') })).toBeVisible();
    await page.getByRole('button', { name: app.t('mt5-add') }).first().click();
    const name = `${prefix} gold demo`;
    await page.getByLabel(app.t('mt5-field-name')).fill(name);
    await page.getByRole('button', { name: app.t('mt5-create'), exact: true }).click();
    await expect(page).toHaveURL(/\/mt5-terminals\/[0-9a-f-]{36}/);
    await expect(page.getByRole('heading', { name: app.t('mt5-connect-title') })).toBeVisible();
    await expect(page.getByText(app.t('mt5-catalogue-empty'))).toBeVisible();

    await app.goto('/mt5-terminals');
    const row = page.getByRole('row').filter({ hasText: name });
    await expect(row).toBeVisible();
    await expect(row).toContainText(app.t('mt5-presence-never'));
  });

  test('worst case: long names, every presence, a full page', async ({ app, page, capture }) => {
    const presences = ['online', 'late', 'offline', 'never'];
    const long = `Terminal ${'with a very long name '.repeat(8)}end`;
    await page.route('**/api/v1/mt5/terminals?*', (r) =>
      r.fulfill({
        json: {
          count: 45,
          next: null,
          previous: null,
          results: Array.from({ length: 20 }, (_, i) => terminal(i, i === 0 ? long : `T${i}`, presences[i % 4])),
        },
      }),
    );
    await app.goto('/mt5-terminals');
    for (const p of presences) {
      await expect(page.getByText(app.t(`mt5-presence-${p}`)).first()).toBeVisible();
    }
    await expect(page.getByText(app.t('mt5-selection-all')).first()).toBeVisible();
    // The long name wraps inside its row, never past the page.
    const width = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(width).toBeLessThanOrEqual(1);
    await capture('mt5-terminals', { caption: 'MetaTrader 5 terminals and their presence' });
  });
});
