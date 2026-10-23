---
id: open-data-ingestion
title: Open-data ingestion — a visible chain of flow steps
kind: howto
pages: /flows
nodes: ingest, inject
err_codes: ingest-staging-quota, ingest-staging-unavailable, ingest-no-resource, ingest-entry-missing, ingest-archive-unsupported, ingest-unsafe-path, ingest-suspicious-closures, import-run-interrupted, import-schema-changed, import-source-unreachable, import-source-too-large, import-source-unreadable
tags: ingestion, open data, données ouvertes, data.gouv, france compétences, télécharger, download, zip, archive, extraire, csv, filtre, filter, colonnes, columns, staging, zone de transit, inchangé, unchanged, import, rncp, rs, certificateurs, partenaires, habilitation, compétences, competencies, blocs, référentiel, programme, module, xml
---
A flow can fetch an open-data file by itself every night and feed the ontology, one **visible step per node** (palette section *Data ingestion*). Each step shows, in its panel under **Last runs**, what it read and what it produced: the archive's table of contents, the columns with their fill rate and examples, the rows kept by a filter, the counters of the import.

## The steps, in order
1. **Open source**: a fixed URL, or a *data.gouv.fr dataset* (its identifier) and a pattern on the resource title such as `export-fiches-csv-*.zip`: the most recent matching resource wins, so a file whose name changes every day is found.
2. **Download**: the file goes to the server's staging area, never to a disk. When it is the same as on the last successful run, the node leaves through its third output (*unchanged*) and the chain stops there.
3. **Archive extraction**: zip, tar, tar.gz or gz (no rar). List the files to keep, one pattern per line (`*Partenaires*.csv`); the panel lists every file of the archive, kept or not.
4. **Table inspection**: format, encoding, separator, exact number of rows, each column with its fill rate, distinct values and examples, and five sample rows. List the **expected columns**: if the source renames or drops one, the chain stops (*import-schema-changed*) and nothing is imported.
5. **Row filter**: keeps the rows whose column equals a value, is in a list, or is among the values of a property of your objects (for example the codes of your certifications). *Only objects whose property…* narrows those objects (for example status *actif*: the partners of your active certifications only, which keeps a large partner file under the limit). Conversions such as *digits* or *trim* clean the column before comparing. It runs before the import limit of 250,000 records.
6. **Ontology import**: runs one of your imports (Ontology › Imports) on the file, as a *preview* (nothing written, applicable later from the Imports tab) or *applied*. It refuses a run that would close more than the allowed share of what the import holds (*ingest-suspicious-closures*, 20 % by default). An API import (company register) runs on its own source.
7. **Cleanup**: deletes the run's extracted and filtered files; the downloaded file stays for its retention time (30 days by default) as proof of what was imported.

Each node has a second output for **errors** (the message carries the error code): wire it to a notification to hear about a broken source. Several tables of one archive are chained one after the other, each step picking its file with the **File read** pattern.

## France Compétences (pack *Partner network watch*)
The pack ships the draft flow **Ingestion France Compétences**: certifiers, sheets, partners, then the company register. Before deploying it, open the *Certificateurs* row filter and put **your certifier's SIRET** in its values: the flow then imports your certifications, their sheets (title, level, dates, status), your partners and their accreditation scope (preparation, evaluation or both). A partner gone from the open data loses its accreditation; nothing is deleted.

The same night it reads the **content of the sheets**: the skill blocks of your RNCP certifications, then, from the RS register (XML, a second download), each **competency** of your RS certifications (one per paragraph of the official framework, keyed like `RS7511-C3`), their objectives, prerequisites, jury and number of graduates, and the France Compétences **state** of each accreditation (active or inactive, since when). Only your certifications are enriched; an accreditation is never opened from the RS register, only completed. A revised framework closes the competencies it no longer lists. Programmes and modules (the reference programme and each partner's) are compared to these competencies through *covers* links.

## When it goes wrong
- *ingest-no-resource*: no resource of the dataset matches the pattern — open the source node and check the pattern against the dataset page.
- *ingest-entry-missing*: no extracted file matches the *File read* pattern of the step — compare it with the table of contents shown by the extraction node.
- *import-schema-changed* on an inspection: the source renamed a column — the inspection panel lists the columns it found; update the expected columns and the import's column mapping.
- A long import (thousands of rows, or a company register enrichment) keeps its node busy until it ends, minutes or hours: the node status stays on that step meanwhile, and the Imports tab shows the run with its record count.
- *import-run-interrupted*: the server running the import stopped (restart, update); nothing of that run was written. Run the flow again from its trigger.
- *ingest-suspicious-closures*: the source suddenly lost much of what the import holds — check the file in the inspection panel; if the drop is real, raise the allowed closures on the import node for one run.
- *ingest-staging-quota*: the organization's staging area is full (2 GB) — add a Cleanup node at the end of the chain; files older than 7 days are purged anyway.
- *ingest-archive-unsupported* or *ingest-unsafe-path*: the file is not an archive that can be read, or it holds a path leaving its folder — it is refused as a whole.
