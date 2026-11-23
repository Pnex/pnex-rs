---
id: open-data-ingestion
title: Open-data ingestion — a pipeline, then an import
kind: howto
pages: /ontology
nodes: dataset_trigger
err_codes: connector-not-found, connector-unreachable, connector-host-refused, connector-query-invalid, build-cycle, build-dependency-missing, build-too-long, build-author-forbidden, build-timeout, pipeline-check-failed, dataset-not-found, ingest-staging-unavailable, ingest-entry-missing, ingest-suspicious-closures, import-run-interrupted, import-schema-changed, import-source-unreachable, import-source-too-large, import-source-unreadable, flow-node-retired, flow-node-unreadable
tags: table trigger, déclencheur de table, dataset_trigger, chained run, planification, schedule, dépendances, dependencies, after, ingestion, open data, données ouvertes, data.gouv, france compétences, télécharger, download, zip, archive, extraire, csv, filtre, filter, colonnes, columns, staging, zone de transit, import, rncp, rs, certificateurs, partenaires, habilitation, compétences, competencies, blocs, référentiel, programme, module, xml, étapes, steps
---
To fetch an open-data file every night, write a **pipeline** (Pipelines): a `fetch` step downloads it, `unpack` opens an archive (zip, tar, tar.gz or gz) and keeps the files you list (`*Partenaires*.csv`), `read` turns a file into rows and checks its expected columns, `sql` filters them, and `write` keeps them in a **table**. Schedule the pipeline on its **Schedule** tab. An import (Ontology › Imports) whose source is that table then turns its rows into objects. An import no longer carries steps of its own.

The import applies its mapping. A run that would close more than 20 % of what the import holds is refused (*ingest-suspicious-closures*).

## France Compétences (pack *Partner network watch*)
The pack ships its France Compétences imports with their pipelines: certifiers, sheets, partners, skill blocks and the RS register (XML). Only the last one is scheduled, every night: each import names the ones it waits for (`after:` in its mapping), so the others run first, in order. If one fails, what waits for it is not run and keeps its data, and the organization gets a notification. An import that already had its own schedule keeps it after a pack upgrade: remove that schedule to leave only the last one. Put **your certifier's SIRET** in the pack setting **Certifiers followed** (Ontology › Packs): the certifiers import keeps only those, then the others keep the rows of your certifications. You get your certifications, their sheets (title, level, dates, status), your partners and their accreditation scope (preparation, evaluation or both). A partner gone from the open data loses its accreditation; nothing is deleted.

The RS register gives each **competency** of your RS certifications (one per paragraph of the official framework, keyed like `RS0101-C3`), their objectives, prerequisites, jury and number of graduates, and the France Compétences **state** of each accreditation. Only your certifications are enriched; a revised framework closes the competencies it no longer lists.

## When it goes wrong
- *ingest-entry-missing*: the import has no source to read — give it a table (or a file with **Preview**).
- *import-schema-changed*: the source renamed a column — the run's steps list the columns found; update the expected columns and the import's column mapping.
- *import-run-interrupted*: the server running the import stopped (restart, update); nothing of that run was written. Use **Run now** or wait for the next night.
- *ingest-suspicious-closures*: the source suddenly lost much of what the import holds — check the table; if the drop is real, run the import from a file with **Preview**, then apply.
- *flow-node-retired* when deploying a flow: it still holds an old *Data ingestion* node, which no longer exists. Delete that node (or the flow) and write a pipeline instead.
- *flow-node-retired* or *flow-node-unreadable* when opening a flow: the editor left that node out of the canvas. Save the flow to drop it for good, or delete the flow from the Flows list.

## Reading another database
A pipeline reads an outside database with a `connector` step: an admin saves a **connector** (address, user, password in the vault) on the Connectors tab of Pipelines, then the step names it with one read-only query. Its result is a CSV file for a `read`, then a table with the same versions, schedule and checks as any other. A private address needs the platform administrator's approval for that host.

## Reacting to new data in a flow
A **Table trigger** node starts a flow when a table gets a new version, or when a sync from that table is applied. Name the table in the node: it must exist when you deploy. The message says which version and how many rows, or which objects changed, never the rows themselves. Events can arrive up to a few tens of seconds after the data, and those that come while the flow is stopped are not replayed.

## Troubleshooting
- **Deploy refused, the table does not exist**: the name in the Table trigger node must match a table of the organization exactly. A pipeline creates its table at its first run: run it once, then deploy.
- **A chained run stopped, with one import in failure and the rest blocked**: open that import's history and fix what it reports. If it stopped at a preview because its mapping changed, review and apply the preview, then the next run goes through.
- **A chained run fails after 12 hours**: a step did not finish. For a pipeline, this usually means no worker is taking the queue.
- **"No connection to this database"**: check the connector's address, user, password and database name. **"This address cannot be reached"**: it is private, or TLS is off: ask the platform administrator to allow that host.
- **"The author of this import can no longer use connectors"**: an owner or admin must save the import again, it then runs with their rights.
- **"A check refused the result"**: the new version of the table did not meet a check of its pipeline. The previous version stays in use. The failed check and the measured value are shown on the version.
