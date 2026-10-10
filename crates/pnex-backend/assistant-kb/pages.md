---
id: pages
title: Pages
kind: feature
pages: /pages
nodes: 
err_codes: docs-write-forbidden
tools: 
tags: page, pages, document, notes, procedure, report, collaborative, rich text, markdown, version, note, compte rendu, procédure
---
Pages (menu entry right under Visualisation) are documents of the organization written together, live: procedures, incident reports, meeting notes, project logs. Every member of the organization sees every page.

## What you can do

- **New page**: give it a title; the editor opens at once. Rename it by editing the title at the top.
- Write rich text: headings, bulleted and numbered lists, quotes, code blocks, dividers, bold, italic, inline code and links. Use the toolbar, or type `/` in an empty line to open the block palette. Markdown shortcuts work too (`# `, `- `, `1. `, `> `, three backticks).
- **Live collaboration**: several people edit the same page at once; their cursors appear with their names, and the header shows how many others are on the page. The status dot says whether changes are saved; offline edits are sent when the connection comes back.
- **Versions**: name the current content to freeze it ("validated by X"), view an old version, compare a version with the previous one, or restore it. Restoring is a normal edit (it can be undone) and creates a new version that says where it came from; nothing is ever erased.
- **Markdown**: export a page as a `.md` file, or import a `.md` file, which is appended at the end of the page.

## Good to know

- Links may only point to http, https, mailto or to a page of the app (`/…`); other links are removed.
- Pasted web content is cleaned: only text, headings, lists, quotes, code and links are kept.
- Images, maps, charts, mentions of devices and tables in pages come in later versions.
- Viewers read pages live but cannot change them.
- If the status says the change was refused, reload the page: the content you typed did not fit the page format.
