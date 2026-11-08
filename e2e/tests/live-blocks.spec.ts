// Live blocks (analysis.md D831–D832): a value computed in a page when it is
// read, with the reader's rights; dated in the PDF of a version.
import { expect, test } from '../src/fixtures.ts';
import { DocsPage } from '../src/pages/docs.ts';

test.describe('live blocks', { tag: '@pages' }, () => {
  test('a calculation on objects shows in the page and in its PDF', async ({ app, api, prefix }) => {
    const tag = Date.now().toString(36);
    const typeKey = `asset${tag}`;
    const typeName = `Asset ${tag}`;
    await api.post('/ontology/types', { def: { key: typeKey, name: typeName, properties: [] } });
    for (const title of ['Pump A', 'Pump B', 'Valve C']) {
      await api.post('/ontology/objects', { type_key: typeKey, title, properties: {} });
    }

    const docs = new DocsPage(app);
    await docs.open();
    const editor = await docs.create(`${prefix} live ${tag}`);
    await editor.typeAtEnd('/' + app.t('docs-cmd-live').split(' ')[0].toLowerCase());
    await app.page.locator('.pnex-doc-menu-item').filter({ hasText: app.t('docs-cmd-live') }).click();

    const dialog = app.page.getByRole('dialog', { name: app.t('live-settings') });
    await dialog.getByLabel(app.t('live-title')).fill('Assets on site');
    await dialog.getByLabel(app.t('live-source-objects')).check();
    await dialog.getByLabel(app.t('rules-type')).selectOption({ label: typeName });
    await dialog.getByRole('button', { name: app.t('pipe-save'), exact: true }).click();
    await expect(dialog).toBeHidden();

    const block = editor.content.locator('.pnex-live');
    await expect(block.locator('.pnex-live-title')).toHaveText('Assets on site');
    await expect(block.locator('.pnex-live-figure')).toHaveText('3');
    await expect(block.locator('.pnex-live-caption')).toHaveText(app.t('live-op-count'));

    // A new object shows after "Refresh".
    await api.post('/ontology/objects', { type_key: typeKey, title: 'Valve D', properties: {} });
    await block.getByRole('button', { name: app.t('live-refresh') }).click();
    await expect(block.locator('.pnex-live-figure')).toHaveText('4');

    // The PDF of a version computes the block at the export.
    await editor.waitSynced();
    const versions = await editor.openVersions();
    await versions.getByRole('textbox', { name: app.t('docs-version-label') }).fill('with block');
    await versions.getByRole('button', { name: app.t('docs-version-save') }).click();
    await expect(versions.getByText('v1 · with block')).toBeVisible();
    const pdf = await api.download(`/docs/${editor.id}/versions/1/pdf?lang=en`);
    expect(pdf.subarray(0, 5).toString()).toBe('%PDF-');
  });
});
