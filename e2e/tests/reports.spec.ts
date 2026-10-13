// Reports (media-vision-studio.md D209/D210): a flow writes an AI version of
// a page, a person approves it in Pages, then downloads its PDF. The flow
// side is played through the internal route, so the test needs the server's
// runtime token in PNEX_E2E_FLOW_TOKEN (same value as PNEX_FLOW_RUNTIME_TOKEN).
import { expect, test } from '../src/fixtures.ts';
import { DocEditor } from '../src/pages/docs.ts';

test.describe('reports', { tag: ['@pages'] }, () => {
  test('an AI version is approved, then exported as PDF', async ({ app, api, page, prefix, capture }) => {
    const token = process.env.PNEX_E2E_FLOW_TOKEN ?? '';
    test.skip(!token, 'PNEX_E2E_FLOW_TOKEN needed (server runtime token)');
    const base = (process.env.PNEX_E2E_BASE_URL ?? '').replace(/\/$/, '');
    const res = await page.request.post(`${base}/internal/flow/page-write`, {
      headers: { 'x-pnex-flow-token': token },
      data: {
        org_id: api.orgId,
        flow_id: 1,
        mode: 'new_page',
        title: `${prefix} weekly report {week}`,
        markdown: '# Week summary\n\nThe line ran **without stops**.\n\n- 3 alerts\n- 1 visit',
        version_label: '{date}',
        ai_generated: true,
      },
    });
    expect(res.status(), await res.text()).toBe(201);
    const written: { page_id: string; version: number } = await res.json();

    await app.goto(`/pages?id=${written.page_id}`);
    const editor = new DocEditor(app, page);
    await editor.waitSynced();
    await expect(editor.content.locator('h1')).toHaveText('Week summary');
    const versions = await editor.openVersions();
    const v = versions.locator('li').filter({ hasText: `v${written.version}` });
    await expect(v.getByText(app.t('docs-version-by-flow'))).toBeVisible();
    await expect(v.getByText(app.t('docs-version-ai-review'))).toBeVisible();
    await capture('report-ai-version', { caption: 'Pages: a report version written by a flow, AI text to review' });

    await v.getByRole('button', { name: app.t('docs-version-approve') }).click();
    await expect(v.getByText(app.t('docs-version-ai-review'))).toBeHidden();
    await expect(v.getByRole('button', { name: app.t('docs-version-approve') })).toBeHidden();
    await capture('report-approved', { caption: 'Pages: the report version approved by a person' });

    const download = page.waitForEvent('download');
    await v.getByRole('button', { name: app.t('docs-version-pdf') }).click();
    const file = await download;
    expect(file.suggestedFilename()).toMatch(/\.pdf$/);
    const path = await file.path();
    const { readFileSync } = await import('node:fs');
    expect(readFileSync(path).subarray(0, 5).toString()).toBe('%PDF-');
  });
});
