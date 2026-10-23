---
id: ontology-imports
title: Ontology imports — sheets, files and APIs into objects
kind: feature
pages: /ontology
err_codes: ingest-entry-missing, import-source-unreadable, import-source-too-large, import-source-unreachable, import-schema-changed, import-suspicious-drop, import-has-errors, import-run-stale, ontology-unique-duplicates, ontology-version-conflict
tags: import, imports, excel, xlsx, csv, json, xml, zip, api, sync, synchronisation, synchronization, mapping, correspondance, column, colonne, preview, aperçu, snapshot, liste complète, schedule, calendrier, siren, siret, unique, clé naturelle, natural key, register, registre, recherche d'entreprises, réseau, partenaires, habilitation, hierarchy, hiérarchie, run now, lancer maintenant, steps, étapes, pipeline, data.gouv
---
Ontology › **Imports** turns the rows of a sheet (Excel, CSV), a file at an address (CSV, XML, JSON, possibly zipped) or a JSON API into objects and links. Nothing is written before you have seen the preview and pressed **Apply**.

## How an import works
- An import has a **column mapping** (YAML): where the records are, which type each one creates, the **unique property** that recognizes an object already there (for example the SIREN of an organization), which column feeds which property, and which links to open between the objects of a row. No code: columns, constants and a fixed list of conversions (trim, digits only, first N characters, date format, amount in a currency, split a list, one value per paragraph, text before or after a separator, website address, domain, SIREN/SIRET check, e-mail check, value lists). Several rows of one pair disagreeing on a link attribute keep the first value with a *conflict* warning, unless the link says which value to take (`on_conflict`).
- A row can expand a list (`each`: one object per element, its position is `#`). An object can **need** another one of the row (`needs`): when that one is not in the organization, nothing of it is written — a whole public register read without a filter only enriches your own records. A link marked `create: false` only completes links already open (for example the state of an accreditation), it never opens one.
- A property can say `invalid: skip`: a value its conversion or the type refuses (a SIREN failing its check digit, an e-mail, a date, a text over its maximum length) becomes a **warning** of the run instead of an error that refuses a *Full list* import. The property is left out of that row and the object keeps its current value. When it is the object's key, the object is left out of that row with the links that need it; in *Full list* mode the links the import already opened on the other end (for example the certification) are neither closed nor updated during that run, as the run cannot tell which one the row meant. Without it, an invalid value stays an error (*import-has-errors* on a scheduled *Full list* run).
- A type property is **unique** when *Unique* is ticked in the type editor: one object per value. Ticking it on a type whose objects already share values is refused (*ontology-unique-duplicates*, with the first values shared): fix those objects first.
- Packs may ship imports: installing *Partner network watch* adds *Partner network (sheet)*; *French companies* adds the company register sync.

## Preview, then apply
1. **Preview** (send the file for a sheet import): the preview lists what would be **created**, **updated** (old → new value), **closed**, the **links** opened, and the **errors** by row and column. Rows count as in the sheet (row 2 is the first one under the headers).
2. **Apply** writes exactly what the preview showed, all at once. A preview with errors cannot be applied (*import-has-errors*): fix the file and preview again.
3. *import-run-stale*: the preview is out of date (already applied, the import was edited, or the objects changed since): preview again.

## Modes
- **Add and update**: creates and updates, never closes anything.
- **Full list**: in addition, what this import wrote and the source no longer holds is **closed** (a link gets an end date, an object is archived): a partner removed from the sheet loses its accreditation at the import date. Nothing is ever deleted.

## Schedules (file at an address, API, or an import with its own steps)
- A schedule (every day, week or month) runs the import at night and applies it without preview. An import of a sent file can be scheduled once it has its own steps (it then fetches its file itself); without steps it cannot.
- It is **refused without writing anything** when the source changed shape (*import-schema-changed*: missing columns, or no record while the last run had some) or, in *Full list* mode, when it lost more than 30 % of its records (*import-suspicious-drop*). You get an in-app notice; the history keeps every run with its counters.
- *import-source-unreachable*: the address cannot be reached (refused internal address, network, server error). *import-source-unreadable*: the file is not in the expected format. *import-source-too-large*: over 200 MB once decompressed, 250,000 records or 500,000 objects (lists expanded) per run.

## Run now, and imports with their own steps
- **Run now** (a writer, on an import that has something to fetch) starts a run at once, like a scheduled one: checked, then applied without preview, with the author's role. Follow it in **History**.
- An import can carry its own **steps** in its YAML, under `steps:` (the same as the flow *Open-data ingestion* nodes: find the newest file of a data.gouv.fr dataset, download, extract an archive, inspect a table, filter rows). *Run now* runs them on the server, then applies the mapping to the file they leave. Large files are written in bulk: tens of thousands of rows take about a minute.
- *import-source-too-large* on *Run now* with steps: the filtered file still holds more than 250,000 rows. Narrow the row filter, for example `only: {property: statut, values: [actif]}` on an `in_objects` filter keeps the rows of your active objects only.
- **History** shows each run with its outcome and, for a refusal or a failure, the message with its figures (for example how many records the source held against the limit). **Steps** unfolds the steps of a run: outcome, duration, rows read and kept, and the step where it stopped.
- *import-server-stopping*: the server is restarting and starts no run for a minute; run it again. *import-run-interrupted*: the server stopped during the run, which wrote nothing; run it again.
- *ingest-entry-missing* on *Run now*: a sheet import has nothing to fetch; send its file with **Preview** instead, or give it steps.

## Hierarchy
Objects of a pack with hierarchies (for example *French companies*: group › subsidiaries › establishments, and officers) have a **Hierarchy** tab: a tree computed on the links, at a date. A person who directs several companies appears under each one, marked "also under …".

## Good to know
- Viewers see the imports and their history but cannot preview, apply, edit or delete them.
- Deleting an import keeps everything it wrote.
- The assistant may explain a preview; applying an import is always yours.
