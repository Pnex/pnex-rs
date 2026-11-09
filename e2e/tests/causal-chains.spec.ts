// Causal chains and point-in-time views (cases-workspace.md appendix I,
// markets-research.md appendix H): a proposed chain shows its hops and lift,
// a case opened from an alert offers the assistant frozen at the alert
// + 15 min, and the model decisions page opens an analysis at a date. The
// detector itself is covered server side (tests/ontology.rs); here the
// proposal is served.
import { expect, test } from '../src/fixtures.ts';

const ref = (id: string, title: string, type_key: string) => ({ id, title, type_key, native_id: '' });

const chain = (caseId: string) => ({
  id: '00000000-0000-4000-8000-0000000000c1',
  kind: 'causal_chain',
  detector: 'causal_chain@1',
  params: { min_lift: 2 },
  t_start: '2026-10-03T10:00:00Z',
  t_end: '2026-10-03T10:12:00Z',
  refs: [
    ref('00000000-0000-4000-8000-0000000000a1', 'Controller CF-2', 'controller'),
    ref('00000000-0000-4000-8000-0000000000a2', 'Compressor K-7', 'compressor'),
  ],
  payload: {
    hops: [{ rule: 'change.config → alert', gap_s: 720, distance: 1, score: 0.18 }],
    confidence: 0.18,
    placebo: 0,
    lift: 18,
    rank: 1,
    root: '00000000-0000-4000-8000-0000000000a1',
    effect: '00000000-0000-4000-8000-0000000000a2',
  },
  status: 'proposed',
  reject_reason: null,
  event_id: null,
  case_id: caseId,
});

const CONVERSATION = {
  id: '6f1c7a52-6a7e-4d0a-9d55-2a8d3c1b9e02',
  title: '',
  created_at: '2026-11-09T10:00:00Z',
  last_message_at: '2026-11-09T10:00:00Z',
};

async function mockAssistant(page: import('@playwright/test').Page, sent: { body?: Record<string, unknown> }) {
  await page.route('**/api/v1/ai/status', (route) =>
    route.fulfill({ json: { enabled: true, configured: true, provider_name: 'mock' } }),
  );
  await page.route('**/api/v1/ai/conversations', (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    sent.body = route.request().postDataJSON();
    return route.fulfill({ json: { ...CONVERSATION, frozen_at: sent.body?.frozen_at } });
  });
}

test.describe('causal chains', { tag: ['@cases', '@i18n'] }, () => {
  test('a proposed chain shows its hops, and the frame asks the assistant as of the alert', async ({
    app,
    page,
    api,
    prefix,
  }) => {
    const c = await api.post('/cases', {
      title: `${prefix} high pressure`,
      from: { kind: 'event', at: '2026-10-03T10:12:00Z' },
    });
    await page.route(`**/api/v1/cases/${c.id}/proposals`, (route) =>
      route.fulfill({ json: { count: 1, next: null, previous: null, results: [chain(c.id)] } }),
    );
    const sent: { body?: Record<string, unknown> } = {};
    await mockAssistant(page, sent);
    await app.goto(`/cases/${c.id}`);

    const row = page.getByTestId('case-proposal-row').first();
    await expect(row).toContainText(app.t('cw-proposal-causal-chain', { cause: 'Controller CF-2', effect: 'Compressor K-7' }));
    const hops = row.getByTestId('case-chain-hops');
    await expect(hops).toContainText('change.config → alert');
    await expect(hops).toContainText('12 min');
    await expect(row).toContainText('18.0');

    await page.getByTestId('case-frame').click();
    await page.getByTestId('case-ask-frozen').click();
    await expect(page.getByTestId('ai-frozen-banner')).toContainText('2026-10-03 10:27 UTC');
    await page.getByPlaceholder(app.t('ai-placeholder')).fill(`${prefix} what came before?`);
    await page.getByRole('button', { name: app.t('ai-send'), exact: true }).click();
    await expect.poll(() => sent.body?.frozen_at).toBe('2026-10-03T10:27:00+00:00');
  });
});

test.describe('analysis at a date', { tag: ['@markets', '@i18n'] }, () => {
  test('the model decisions page opens a frozen conversation', async ({ app, page, api, prefix }) => {
    await api.post('/ontology/packs/markets/install', {}).catch(() => undefined);
    const sent: { body?: Record<string, unknown> } = {};
    await mockAssistant(page, sent);
    await app.goto('/market-decisions');
    const section = page.getByTestId('decisions-analyse-as-of');
    const ask = section.getByRole('button', { name: app.t('decisions-as-of-ask') });
    await expect(ask).toBeDisabled();
    await section.locator('input[type="datetime-local"]').fill('2026-01-06T10:15');
    await ask.click();
    await expect(page.getByTestId('ai-frozen-banner')).toContainText('2026-01-06 10:15 UTC');
    await page.getByPlaceholder(app.t('ai-placeholder')).fill(`${prefix} gold that day`);
    await page.getByRole('button', { name: app.t('ai-send'), exact: true }).click();
    await expect.poll(() => sent.body?.frozen_at).toBe('2026-01-06T10:15:00+00:00');
    // A date in the future cannot be asked.
    await section.locator('input[type="datetime-local"]').fill('2099-01-01T00:00');
    await expect(ask).toBeDisabled();
  });
});
