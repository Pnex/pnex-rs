// Pages (pages.md #21): a collaborative document written in the browser —
// rich text through Markdown shortcuts, live sync between two tabs,
// persistence, named versions (diff, restore), anchored comments,
// Markdown export, deletion. Checked against the API where it matters.
import { expect, test } from '../src/fixtures.ts';
import { DocsPage, DocEditor } from '../src/pages/docs.ts';

interface DocRow {
  id: string;
  title: string;
}

test.describe('pages', { tag: '@pages' }, () => {
  test('write together, version, comment, export, delete', async ({ app, api, prefix, capture }) => {
    const title = `${prefix} pump procedure`;
    const docs = new DocsPage(app);
    await docs.open();
    const editor = await docs.create(title);
    const id = editor.id;
    expect(id).not.toBe('');

    // Markdown shortcuts become blocks.
    await editor.typeAtEnd('# Replace the seal\n');
    await editor.typeAtEnd('- Isolate the pump\nCheck torque 45 Nm');
    await expect(editor.content.locator('h1')).toHaveText('Replace the seal');
    await expect(editor.content.locator('ul li')).toHaveCount(2);
    await editor.waitSynced();
    await capture('page-editor', { caption: 'A procedure written with Markdown shortcuts' });

    // A second tab sees the edits live, and its edits come back.
    const other = await app.page.context().newPage();
    await other.goto(app.page.url());
    const second = new DocEditor(app, other);
    await second.waitSynced();
    await expect(second.content).toContainText('Check torque 45 Nm');
    await second.typeAtEnd('\nSigned by the second tab');
    await expect(editor.content).toContainText('Signed by the second tab');
    await other.close();

    // Persisted: a reload rebuilds the page from the server.
    await app.page.reload();
    await editor.waitSynced();
    await expect(editor.content).toContainText('Signed by the second tab');

    // Named versions: freeze, change, freeze again, compare, restore.
    let versions = await editor.openVersions();
    await versions.getByRole('textbox', { name: app.t('docs-version-label') }).fill('draft');
    await versions.getByRole('button', { name: app.t('docs-version-save') }).click();
    await expect(versions.getByText('v1 · draft')).toBeVisible();
    await editor.typeAtEnd('\nExtra step');
    await editor.waitSynced();
    await versions.getByRole('textbox', { name: app.t('docs-version-label') }).fill('final');
    await versions.getByRole('button', { name: app.t('docs-version-save') }).click();
    await expect(versions.getByText('v2 · final')).toBeVisible();
    const v2 = versions.locator('li').filter({ hasText: 'v2 · final' });
    await v2.getByRole('button', { name: app.t('docs-version-diff') }).click();
    const diff = app.page.getByRole('dialog', { name: app.tr('docs-version-diff-title') });
    await expect(diff.getByText('Extra step')).toBeVisible();
    await capture('page-version-diff', { caption: 'What changed in version 2' });
    await diff.getByRole('button').first().click();
    await expect(diff).toBeHidden();
    const v1 = versions.locator('li').filter({ hasText: 'v1 · draft' });
    await v1.getByRole('button', { name: app.t('docs-version-restore') }).click();
    await expect(editor.content).not.toContainText('Extra step');
    await expect(versions.getByText(/^v3 · /)).toBeVisible({ timeout: 15_000 });

    // Anchored comment on the last line, reply, resolve.
    await editor.selectLastLine();
    await editor.toolbar('docs-cmd-comment');
    const comments = editor.commentsPanel;
    await comments.getByRole('textbox', { name: app.t('docs-comment-new') }).fill('Which wrench size?');
    await comments.getByRole('button', { name: app.t('docs-comment-post') }).click();
    await expect(comments.getByText('Which wrench size?')).toBeVisible();
    await expect(editor.content.locator('.pnex-doc-comment')).toHaveCount(1);
    await comments.getByRole('textbox', { name: app.t('docs-comment-reply') }).fill('24 mm');
    await comments.getByRole('button', { name: app.t('docs-comment-post') }).click();
    await expect(comments.getByText('24 mm')).toBeVisible();
    await capture('page-comments', { caption: 'A comment thread anchored on the text' });
    const thread = await api.get<{ results: { body: string; resolved: boolean }[] }>(`/docs/${id}/comments`);
    expect(thread.results.map((c) => c.body)).toEqual(['Which wrench size?', '24 mm']);
    await comments.getByRole('button', { name: app.t('docs-comment-resolve') }).click();
    await expect(editor.content.locator('.pnex-doc-comment')).toHaveCount(0);

    // Markdown export.
    const download = app.page.waitForEvent('download');
    await app.page.getByRole('button', { name: app.t('docs-export-md') }).click();
    const md = await (await download).createReadStream();
    let text = '';
    for await (const chunk of md) text += chunk.toString();
    expect(text).toContain('# Replace the seal');
    expect(text).toContain('Check torque 45 Nm');

    // Delete from the list.
    await docs.open();
    await docs.delete(title);
    const left = (await api.list<DocRow>('/docs')).filter((d) => d.title === title);
    expect(left).toHaveLength(0);
  });
});
