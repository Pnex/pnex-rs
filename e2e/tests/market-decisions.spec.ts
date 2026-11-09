// Model decisions (markets-research.md D897): the page lists the decisions of
// external models with their measure, and an instrument's Prices tab draws
// them as dots, one model at a time. Served data, worst case included
// (break-ui): very long model names, many groups, a crowded day.
import { expect, test } from '../src/fixtures.ts';

const day = (i: number) => new Date(Date.UTC(2026, 0, 1) + i * 86_400_000).toISOString().slice(0, 10);

const decision = (i: number, model: string) => ({
  id: `00000000-0000-4000-8000-${String(i).padStart(12, '0')}`,
  model,
  model_version: `v${i % 3}`,
  instrument: 'XAUUSD',
  source: 'histdata',
  interval: '4h',
  horizon_bars: 6,
  as_of: `${day(i % 30)}T08:00:00.000000Z`,
  options: ['buy', 'neutral', 'sell'],
  probabilities: i % 2 ? [0.72, 0.18, 0.1] : [0.1, 0.25, 0.65],
  best: i % 2 ? 'buy' : 'sell',
  confidence: 0.31,
  barriers: { take_profit_atr: 1.5, stop_atr: 1, atr_period: 14 },
  inputs: null,
  created_at: `${day(i % 30)}T08:00:01.000000Z`,
});

const scores = (model: string) => ({
  model,
  model_version: 'v1',
  instrument: 'XAUUSD',
  source: 'histdata',
  interval: '4h',
  scores: {
    decisions: 120,
    measured: 100,
    hit_rate: 0.61,
    brier: 0.512,
    reliability: [
      { from: 0.1, to: 0.2, count: 180, mean_probability: 0.14, observed: 0.2 },
      { from: 0.7, to: 0.8, count: 60, mean_probability: 0.72, observed: 0.65 },
    ],
    threshold: 0.6,
    above_threshold: 60,
    above_threshold_hit_rate: 0.65,
    mean_return: 0.002,
  },
});

test.describe('model decisions', { tag: ['@markets', '@i18n'] }, () => {
  test.beforeEach(async ({ api }) => {
    await api.post('/ontology/packs/markets/install', {}).catch(() =>
      api.post('/ontology/packs/markets/install', {}),
    );
  });

  test('the page lists decisions and measures each model', async ({ app, page, capture }) => {
    const long = `gold-${'very-long-model-name-'.repeat(8)}end`;
    await page.route('**/api/v1/markets/decisions/scores*', (r) =>
      r.fulfill({ json: { count: 2, next: null, previous: null, results: [scores('gold'), scores(long)] } }),
    );
    await page.route('**/api/v1/markets/decisions?*', (r) =>
      r.fulfill({
        json: {
          count: 45,
          next: null,
          previous: null,
          results: Array.from({ length: 20 }, (_, i) => decision(i, i === 0 ? long : 'gold')),
        },
      }),
    );
    await app.goto('/market-decisions');
    await expect(page.getByRole('heading', { name: app.t('decisions-title') })).toBeVisible();
    await expect(page.getByTestId('decisions-advice')).toHaveText(app.t('decisions-advice'));
    const measured = page.getByTestId('decisions-scores');
    await expect(measured.locator('li')).toHaveCount(2);
    await expect(measured).toContainText('61 %');
    await expect(page.getByText(`${app.t('decisions-option-buy')} 72 %`).first()).toBeVisible();
    // The long name wraps inside its row, never past the page.
    const width = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(width).toBeLessThanOrEqual(1);
    await capture('market-decisions', { caption: 'Decisions of an external model and its calibration' });
  });

  test('an instrument draws decisions as dots, one model at a time', async ({ app, page, api, prefix }) => {
    const ticker = `D${Date.now() % 100000}`;
    const inst = await api.post('/ontology/objects', {
      type_key: 'instrument',
      title: `${prefix} ${ticker}`,
      properties: { ticker },
    });
    const closes = Array.from({ length: 60 }, (_, i) => ({ date: day(i), value: 2600 + i }));
    const events = Array.from({ length: 200 }, (_, i) => ({
      id: decision(i, 'gold').id,
      date: day(i % 60),
      kind: 'decision',
      title: i % 2 ? 'gold v1' : 'silver v2',
      detail: i % 3 ? 'buy' : 'sell',
      model: i % 2 ? 'gold v1' : 'silver v2',
      probability: 0.4 + (i % 6) / 10,
    }));
    await page.route('**/api/v1/markets/issuers/*/chart*', (r) =>
      r.fulfill({ json: { instrument_id: inst.id, ticker, closes, events } }),
    );
    await app.goto(`/ontology/object/${inst.id}`);
    await page.getByRole('tab', { name: app.t('onto-tab-chart') }).click();
    const chart = page.getByTestId('onto-chart');
    await expect(chart.getByTestId('onto-chart-mark')).toHaveCount(200);
    await chart.getByTestId('onto-chart-model').selectOption('gold v1');
    await expect(chart.getByTestId('onto-chart-mark')).toHaveCount(100);
    await chart.getByTestId('onto-chart-mark').first().focus();
    await expect(page.getByTestId('onto-chart-picked')).toContainText('gold v1');
  });
});
