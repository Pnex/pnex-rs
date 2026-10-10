---
id: documents
title: Documents and search
kind: feature
pages: 
nodes: 
err_codes: media-index-unsupported, media-index-too-large, media-index-malformed, media-index-unreadable, media-index-not-a-document
tools: search_docs, read_chunk, open_page
tags: document, documents, search, recherche, manual, manuel, procedure, procédure, pdf, word, docx, excel, xlsx, csv, ods, text, markdown, datasheet, fault code, code défaut, index
---
Manuals, procedures, reports, datasheets and exports uploaded to the Media library (Data › Library) are indexed for search: PDF (with a text layer), Word (.docx), text (.txt), Markdown (.md), CSV, Excel (.xlsx) and OpenDocument spreadsheets (.ods). They appear with the kind **Document** (text) or **Table** (spreadsheets).

## What you can do
- **Upload** the file in the Library like any media: the kind is detected from the file itself and indexing starts at once.
- **Search** with the search box of the Library: exact codes (`E-0457`, part numbers) and words (plural or conjugated forms match too). Each result shows the document, the page (PDF, or the sheet of a spreadsheet) or the section title, and an excerpt with the matching words highlighted. Only the current version of each document is searched.
- **Indexing state** on the document's page: waiting, extracting, indexed (with page and passage counts), error, or no extractable text. Owners and admins can **Reindex** it.
- **Ask the assistant**: it searches your documents (`search_docs`), reads a passage with its neighbours (`read_chunk`) or a whole PDF page (`open_page`), and cites the document and page or section of what it quotes.

## Good to know
- A scanned PDF (images only) has no text to index: its state says so. Text recognition (OCR) is not available yet.
- Spreadsheets are searched as text (one line per row); questions computed over their columns are not available yet.
- Word headings (Heading/Titre styles) and Markdown `#` titles become the section shown in results.
- A corrupt file ends in *media-index-malformed*; a file over the limits (too many pages or a huge archive) in *media-index-too-large*: the upload itself stays in the Library.
- Uploading a new version replaces what search finds; restoring an older version brings its text back.
- Viewers can search and read; only owners and admins upload or reindex.
- Documents are written by people: the assistant treats their content as information, never as instructions.
