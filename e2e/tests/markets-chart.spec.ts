// Issuer price chart (markets-research.md D813): the Prices tab of an
// instrument, its empty state on a fresh instrument, then the drawing with
// sessions and filings, and the worst case (break-ui): a long history, a
// flat line, very long titles and markers on the same days.
import { expect, test } from '../src/fixtures.ts';

const day = (i: number) => new Date(Date.UTC(2020, 0, 1) + i * 86_400_000).toISOString().slice(0, 10);

test.describe('markets chart', { tag: ['@markets', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/markets/install', {}).catch(() =>
      api.post('/ontology/packs/markets/install', {}),
    );
  });

  test('an instrument shows its prices, sessions and filings', async ({ app, page, api, prefix, capture }) => {
    const ticker = `T${Date.now() % 100000}`;
    const inst = await api.post('/ontology/objects', {
      type_key: 'instrument',
      title: `${prefix} ${ticker}`,
      properties: { ticker },
    });

    // No price read yet: the empty state says what to run.
    await app.goto(`/ontology/object/${inst.id}`);
    await page.getByRole('tab', { name: app.t('onto-tab-chart') }).click();
    await expect(page.getByTestId('onto-chart-empty')).toHaveText(app.t('onto-chart-empty'));

    // The drawing, from a served chart.
    const closes = Array.from({ length: 250 }, (_, i) => ({ date: day(i), value: 100 + 10 * Math.sin(i / 20) }));
    const events = [
      // Oldest first, as the server sends them.
      { id: inst.id, date: day(60), kind: 'filing', title: `8-K ${day(60)}`, detail: '8-K' },
      { id: inst.id, date: day(120), kind: 'move', title: `${ticker} -6.2 % on ${day(120)}`, detail: 'down' },
    ];
    await page.route('**/api/v1/markets/issuers/*/chart*', (r) =>
      r.fulfill({ json: { instrument_id: inst.id, ticker, closes, events } }),
    );
    await page.getByTestId('onto-chart-range-1825').click();
    const chart = page.getByTestId('onto-chart');
    await expect(chart.locator('polyline')).toHaveCount(1);
    await expect(chart.getByTestId('onto-chart-mark')).toHaveCount(2);
    await chart.getByTestId('onto-chart-mark').first().focus();
    await expect(page.getByTestId('onto-chart-picked')).toContainText('8-K');
    await capture('markets-chart', { caption: 'Closing prices with an abnormal session and a filing' });

    // Worst case: ten years of a flat line, 200 markers on few days, long titles.
    const flat = Array.from({ length: 2500 }, (_, i) => ({ date: day(i), value: 42 }));
    const crowd = Array.from({ length: 200 }, (_, i) => ({
      id: inst.id,
      date: day(1000 + (i % 5)),
      kind: i % 2 ? 'move' : 'filing',
      title: `${'Very long filing title that never ends '.repeat(6)}${i}`,
      detail: i % 4 ? 'up' : 'down',
    }));
    await page.unroute('**/api/v1/markets/issuers/*/chart*');
    await page.route('**/api/v1/markets/issuers/*/chart*', (r) =>
      r.fulfill({ json: { instrument_id: inst.id, ticker, closes: flat, events: crowd } }),
    );
    await page.getByTestId('onto-chart-range-3650').click();
    await expect(chart.getByTestId('onto-chart-mark')).toHaveCount(200);
    await chart.getByTestId('onto-chart-mark').last().focus();
    const picked = page.getByTestId('onto-chart-picked');
    await expect(picked).toContainText('199');
    // The long title wraps inside the panel, never past it.
    const [panel, line] = await Promise.all([chart.boundingBox(), picked.boundingBox()]);
    expect(line!.x + line!.width).toBeLessThanOrEqual(panel!.x + panel!.width + 1);
    await capture('markets-chart-worst', { caption: 'Worst case: flat decade, crowded markers' });
  });
});
