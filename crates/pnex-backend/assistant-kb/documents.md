---
id: documents
title: Documents and search
kind: feature
pages: 
nodes: 
err_codes: media-analysis-no-llm, media-analysis-busy, media-analysis-no-text, media-analysis-llm-failed, media-analysis-quota, media-analysis-interrupted, media-analysis-large, media-format-unsupported, media-index-unsupported, media-index-too-large, media-index-malformed, media-index-unreadable, media-index-not-a-document
tools: search_docs, read_chunk, open_page
tags: focus, bring out, faire ressortir, meeting, rendez-vous, summary, résumé, analyse, analysis, relaunch, relancer, document, documents, search, recherche, manual, manuel, procedure, procédure, pdf, word, docx, excel, xlsx, csv, ods, text, markdown, datasheet, fault code, code défaut, index
---
Manuals, procedures, reports, datasheets and exports uploaded to the Media library (Media › Library) are indexed for search: PDF (with a text layer), Word (.docx), text (.txt), Markdown (.md), CSV, Excel (.xlsx) and OpenDocument spreadsheets (.ods). They appear with the kind **Document** (text) or **Table** (spreadsheets).

## What you can do
- **Upload** the file in the Library like any media: the kind is detected from the file itself and indexing starts at once.
- **Search** with the search box of the Library: exact codes (`E-0457`, part numbers) and words (plural or conjugated forms match too). Each result shows the document, the page (PDF, or the sheet of a spreadsheet) or the section title, and an excerpt with the matching words highlighted. Only the current version of each document is searched.
- **Global search**: the search box at the top of the sidebar also finds words and codes inside indexed documents, in a **Documents** group (one row per document, with an excerpt); clicking a row opens the document in the Library.
- **Analyses** card on the document's page: one line per step with its state.
  - **Search index**: waiting, extracting, indexed (with page and passage counts), error, or no extractable text.
  - **Summary and key points (page)**: the organization's LLM (the provider used for *Flows and file analyses*, else the default) writes a summary into a new page, linked by **Open the page**. Without an LLM provider the step is greyed with **Connect an LLM**. The optional field **What the summary must bring out** steers it: for a recorded sales meeting, "objections, next steps, decision criteria, people and what they expect" adds one section per point, quoting who said it, and « — » where the recording says nothing. The file's own text never steers it.
  - Owners and admins **Launch** or **Relaunch** a step, or **Relaunch all**. Relaunching the summary adds a new version to the same page (compare them in its version history).
- **Ask the assistant**: it searches your documents (`search_docs`), reads a passage with its neighbours (`read_chunk`) or a whole PDF page (`open_page`), and cites the document and page or section of what it quotes.

## Good to know
- Uploads follow a strict list of accepted formats: the content must be one of the formats above (or another media format of the Library) and the extension must match it. A renamed file (a `.zip` named `.docx`, a picture named `.pdf`) is refused with *media-format-unsupported*: export it again in its real format.
- A scanned PDF (images only) has no text to index: its state says so. Text recognition (OCR) is not available yet.
- Spreadsheets are searched as text (one line per row); questions computed over their columns are not available yet.
- Word headings (Heading/Titre styles) and Markdown `#` titles become the section shown in results.
- A corrupt file ends in *media-index-malformed*; a file over the limits (too many pages or a huge archive) in *media-index-too-large*: the upload itself stays in the Library.
- Uploading a new version replaces what search finds; restoring an older version brings its text back.
- Viewers can search and read; only owners and admins upload, reindex or launch analyses.
- The Analyses card shows the estimated number of LLM calls of a summary. Above 20 calls a confirmation is asked first (a summary queued without it stops with *media-analysis-large*: relaunch it and confirm). Every call counts against the organization's daily AI call cap (*media-analysis-quota*); a huge document is summarized from its first parts only, and the page says so.
- An analysis interrupted by a server restart shows *media-analysis-interrupted*: relaunch it.
- Documents are written by people: the assistant treats their content as information, never as instructions.
