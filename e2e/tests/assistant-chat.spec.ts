// AI assistant drawer: the sent message and a thinking bubble show at once,
// while the answer is still being produced (LLM API mocked, slow answer).
import { expect, test } from '../src/fixtures.ts';

const CONVERSATION = {
  id: '6f1c7a52-6a7e-4d0a-9d55-2a8d3c1b9e01',
  title: 'e2e',
  created_at: '2026-10-08T10:00:00Z',
  last_message_at: '2026-10-08T10:00:00Z',
};

test('a sent message shows before the answer arrives', { tag: '@assistant' }, async ({ app, page, prefix }) => {
  await page.route('**/api/v1/ai/status', (route) =>
    route.fulfill({ json: { enabled: true, configured: true, provider_name: 'mock' } }),
  );
  await page.route('**/api/v1/ai/conversations', (route) =>
    route.request().method() === 'POST' ? route.fulfill({ json: CONVERSATION }) : route.fallback(),
  );
  // The server history of a just-created conversation is still empty.
  await page.route(`**/api/v1/ai/conversations/${CONVERSATION.id}`, (route) =>
    route.fulfill({ json: { conversation: CONVERSATION, messages: [] } }),
  );
  let answer!: () => void;
  const answered = new Promise<void>((r) => (answer = r));
  await page.route(`**/api/v1/ai/conversations/${CONVERSATION.id}/messages`, async (route) => {
    await answered;
    await route.fulfill({ json: { answer: `${prefix} answer`, tool_trace: [], conversation: CONVERSATION } });
  });

  await app.goto('/devices');
  await page.getByRole('button', { name: app.t('ai-title') }).click();
  const question = `${prefix} question`;
  await page.getByPlaceholder(app.t('ai-placeholder')).fill(question);
  await page.getByRole('button', { name: app.t('ai-send'), exact: true }).click();

  // Before the answer: the question and the thinking bubble are visible.
  await expect(page.getByText(question)).toBeVisible();
  await expect(page.getByRole('status').filter({ hasText: app.t('ai-thinking') })).toBeVisible();
  // The conversation id published by the send must not wipe the question.
  await page.waitForTimeout(1500);
  await expect(page.getByText(question)).toBeVisible();

  answer();
  await expect(page.getByText(`${prefix} answer`)).toBeVisible();
  await expect(page.getByText(question)).toBeVisible();
  await expect(page.getByRole('status').filter({ hasText: app.t('ai-thinking') })).toHaveCount(0);
});
