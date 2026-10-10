// Document search (doc-search.md P1): real Word / PDF / Excel files are
// uploaded, indexed by the worker and found by the library search; a file
// whose extension lies about its content is refused.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '../src/fixtures.ts';
import { dialog } from '../src/pages/shell.ts';

const DOCS = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'fixtures', 'docs');

test.describe('documents', { tag: ['@media', '@i18n'] }, () => {
  test('documents: indexed, searched, fake extension refused', async ({ app, page, prefix, capture }) => {
    test.setTimeout(120_000);
    const main = page.getByRole('main');

    async function upload(file: string, name: string, buffer = readFileSync(path.join(DOCS, file))) {
      await main.getByRole('button', { name: app.t('media-upload') }).click();
      const dlg = dialog(page, app.t('media-upload-title'));
      await dlg.locator('input[type=file]').setInputFiles({ name: file, mimeType: 'application/octet-stream', buffer });
      await dlg.getByRole('textbox', { name: app.t('media-upload-name') }).fill(name);
      await dlg.getByRole('button', { name: app.t('media-upload'), exact: true }).click();
      return dlg;
    }

    await app.goto('/media');
    for (const file of ['manuel.docx', 'manuel.pdf', 'mesures.xlsx']) {
      const dlg = await upload(file, `${prefix} ${file}`);
      await expect(dlg).toBeHidden({ timeout: 30_000 });
    }
    await expect(main.getByRole('cell', { name: app.t('media-kind-document'), exact: true }).first()).toBeVisible();
    await expect(main.getByRole('cell', { name: app.t('media-kind-table'), exact: true }).first()).toBeVisible();

    // An executable renamed .pdf: refused with the localized machine code.
    const dlg = await upload('evil.pdf', `${prefix} evil`, Buffer.from('MZ\x90\x00 not a pdf'));
    await app.expectToast(app.t('err-media-format-unsupported'));
    await dlg.getByRole('button', { name: app.t('common-cancel') }).click();
    await app.clearToasts();

    // Exact fault code: found in the Word and the PDF manual (indexing runs
    // in the worker, so search again until both are there).
    const search = page.getByPlaceholder(app.t('media-doc-search-placeholder'));
    const hit = (name: string) => main.getByRole('button').filter({ hasText: `${prefix} ${name}` });
    await expect(async () => {
      await search.fill('E-0457');
      await search.press('Enter');
      await expect(hit('manuel.docx')).toBeVisible({ timeout: 2_000 });
      await expect(hit('manuel.pdf')).toBeVisible({ timeout: 2_000 });
    }).toPass({ timeout: 60_000 });
    await expect(hit('manuel.docx').locator('mark', { hasText: 'E-0457' })).toBeVisible();
    await capture('documents-search', { caption: 'Library search: a fault code found in a Word and a PDF manual' });

    // Spreadsheet cell, then stemmed prose (plural form of the source word).
    await search.fill('T-88');
    await search.press('Enter');
    await expect(hit('mesures.xlsx')).toBeVisible();
    await search.fill('vibrations');
    await search.press('Enter');
    await expect(hit('manuel.docx')).toBeVisible();

    // The hit opens the document: indexed state, then reindex.
    await hit('manuel.docx').click();
    await expect(page.getByText(app.t('media-index-status-indexed'))).toBeVisible();
    await capture('documents-index-state', { caption: 'Indexing state of a Word document' });
    await page.getByRole('button', { name: app.t('media-index-reindex') }).click();
    await app.expectToast(app.t('media-index-reindex-queued'));
  });
});
