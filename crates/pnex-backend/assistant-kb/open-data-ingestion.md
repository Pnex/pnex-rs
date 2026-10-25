---
id: open-data-ingestion
title: Open-data ingestion — the steps of an import
kind: howto
pages: /ontology
err_codes: ingest-staging-quota, ingest-staging-unavailable, ingest-no-resource, ingest-entry-missing, ingest-archive-unsupported, ingest-unsafe-path, ingest-suspicious-closures, import-run-interrupted, import-schema-changed, import-source-unreachable, import-source-too-large, import-source-unreadable, flow-node-retired, flow-node-unreadable
tags: ingestion, open data, données ouvertes, data.gouv, france compétences, télécharger, download, zip, archive, extraire, csv, filtre, filter, colonnes, columns, staging, zone de transit, import, rncp, rs, certificateurs, partenaires, habilitation, compétences, competencies, blocs, référentiel, programme, module, xml, étapes, steps
---
An import (Ontology › Imports) can fetch an open-data file by itself every night: its YAML lists its **steps** under `steps:`, the server runs them, then applies the mapping to the file they leave. **History** unfolds each run's steps: what each one read and produced (the archive's table of contents, the columns with their fill rate and examples, the rows kept by a filter) and the step where a run stopped.

## The steps, in order
1. **Open source** (`source_resolve`): a fixed URL, or a *data.gouv.fr dataset* (its identifier) and a pattern on the resource title such as `export-fiches-csv-*.zip`: the most recent matching resource wins, so a file whose name changes every day is found.
2. **Download** (`file_download`): the file goes to the server's staging area, never to a disk. It stays for its retention time (30 days by default) as proof of what was imported.
3. **Archive extraction** (`archive_extract`): zip, tar, tar.gz or gz (no rar). List the files to keep (`*Partenaires*.csv`); the step lists every file of the archive, kept or not.
4. **Table inspection** (`table_inspect`): format, encoding, separator, exact number of rows, each column with its fill rate, distinct values and examples. List the **expected columns**: if the source renames or drops one, the run stops (*import-schema-changed*) and nothing is imported.
5. **Row filter** (`rows_filter`): keeps the rows whose column equals a value, is in a list (or in the codes of a pack setting, `setting:`), or is among the values of a property of your objects (for example the codes of your certifications). `only:` narrows those objects (for example status *actif*). Conversions such as *digits* or *trim* clean the column before comparing. It runs before the import limit of 250,000 records.

The import then applies its mapping. A run that would close more than 20 % of what the import holds is refused (*ingest-suspicious-closures*). The run's extracted and filtered files are deleted when it ends.

## France Compétences (pack *Partner network watch*)
The pack ships its France Compétences imports with their steps, scheduled every night: certifiers, sheets, partners, skill blocks and the RS register (XML). Put **your certifier's SIRET** in the pack setting **Certifiers followed** (Ontology › Packs): the certifiers import keeps only those, then the others keep the rows of your certifications. You get your certifications, their sheets (title, level, dates, status), your partners and their accreditation scope (preparation, evaluation or both). A partner gone from the open data loses its accreditation; nothing is deleted.

The RS register gives each **competency** of your RS certifications (one per paragraph of the official framework, keyed like `RS0101-C3`), their objectives, prerequisites, jury and number of graduates, and the France Compétences **state** of each accreditation. Only your certifications are enriched; a revised framework closes the competencies it no longer lists.

## When it goes wrong
- *ingest-no-resource*: no resource of the dataset matches the pattern — check the pattern of the *Open source* step against the dataset page.
- *ingest-entry-missing*: no extracted file matches the pattern of a step — compare it with the table of contents of the extraction step in History.
- *import-schema-changed*: the source renamed a column — the inspection step lists the columns it found; update the expected columns and the import's column mapping.
- *import-run-interrupted*: the server running the import stopped (restart, update); nothing of that run was written. Use **Run now** or wait for the next night.
- *ingest-suspicious-closures*: the source suddenly lost much of what the import holds — check the file in the inspection step; if the drop is real, run the import from a file with **Preview**, then apply.
- *ingest-staging-quota*: the organization's staging area is full (2 GB) — files older than 7 days are purged anyway.
- *ingest-archive-unsupported* or *ingest-unsafe-path*: the file is not an archive that can be read, or it holds a path leaving its folder — it is refused as a whole.
- *flow-node-retired* when deploying a flow: it still holds an old *Data ingestion* node, which no longer exists. Delete that node (or the flow) and give the import its steps instead.
- *flow-node-retired* or *flow-node-unreadable* when opening a flow: the editor left that node out of the canvas. Save the flow to drop it for good, or delete the flow from the Flows list.
