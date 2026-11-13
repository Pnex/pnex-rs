---
id: pipelines
title: Pipelines, tables and connectors
kind: feature
pages: /pipelines, /pipelines/:id, /pipelines/tables/:id
nodes: dataset_trigger
err_codes: pipeline-not-found, pipeline-name-taken, pipeline-version-conflict, pipeline-document-invalid, pipeline-run-active, pipeline-input-missing, pipeline-input-empty, pipeline-sql-invalid, pipeline-memory-exceeded, pipeline-timeout, pipeline-worker-lost, pipeline-check-failed, build-not-found, build-cycle, build-too-long, build-author-forbidden, build-timeout, dataset-name-taken, dataset-reserved, dataset-csv-invalid, dataset-schema-mismatch, dataset-quota, dataset-source-too-large, dataset-unchanged, connector-name-taken, connector-unreachable, connector-host-refused, connector-rest-denied, connector-rest-status, connector-rest-not-json, connector-rest-no-records, connector-rest-host-changed, connector-rest-page-limit, hook-signature-invalid, hook-body-invalid, hook-inbox-full, hook-secret-missing, named-query-not-found
tags: pipeline, table, dataset, transform, traitement, sql, lineage, lignage, version, retention, rétention, check, contrôle, freshness, fraîcheur, schedule, planification, chained run, exécution enchaînée, connector, connecteur, csv, rest, api, pagination, webhook, parquet, training, entraînement, resample, rééchantillonnage, storage, stockage
---
The Pipelines page (Sources › Pipelines) holds versioned **tables**, the **pipelines** that compute them in SQL, the **chained runs** and the **connectors**.

## Tables
A table keeps its past versions. Create one and load a CSV from its page (5 GB at most, sent in parts, then converted in the background): the file becomes a new version, and readers see it only once it is complete. The page shows the first rows with how full each column is, each version with its checks (**Read this version** shows the rows as they were), the lineage (what the table reads and what reads it) and the expected freshness: past that many hours without a new version, the organization is warned. A table filled by an import belongs to it, and its page names that import: neither a CSV nor a pipeline can write it (*dataset-reserved*).

A table's storage counts against the organization's room (20 GB unless the platform admin set another value for it): past it a load is refused (*dataset-quota*), the files already written stay.

**Training code** reads a table's Parquet files directly, with an access key that may read tables: it lists the files of a version, then downloads each one whole. Pass the same instant to both calls so the files do not move between them.

## Pipelines
**New pipeline** asks for a name, a table to read and a table to write. The studio shows the tables as a graph, left to right. Click a table to see its first rows; click a table the pipeline writes to edit its **transform**: the tables it reads, each under a name, the SQL query, and its **checks** (minimum and maximum rows, unique columns, share of empty values, fall in rows, same columns). A check that refuses keeps the previous version of the table (*pipeline-check-failed*). **Add a transform** chains a new one after the selected table. The **Code** tab shows the same pipeline as YAML; there, `retain_days:` on a transform sets how many days a replaced version of its table stays readable (1 to 3650, 90 when absent; the latest version always stays). A table loaded from a CSV keeps 90 days. **Versions** opens, compares and restores earlier versions.

**Run** works on the saved version. If the pipeline depends on other pipelines or imports, the page lists what will run first: an API import spends its quota. **Schedule** sets one schedule, on the last table of a chain: what it depends on runs before it, and a failure blocks only what depends on it, which keeps its data. An import whose run was cut by a server restart starts again once by itself.

To turn 1-minute bars into hourly ones, write a transform that groups by instrument and hour: `date_trunc('hour', ts)`, or `date_bin(INTERVAL '5 minutes', ts, TIMESTAMP '1970-01-01T00:00:00Z')` for other steps; take `first_value(open ORDER BY ts)`, `max(high)`, `min(low)`, `last_value(close ORDER BY ts)` and `sum(volume)`, with a **unique** check on instrument and time. A missing minute stays missing: nothing is filled.

Times written as a local wall clock are converted in the transform, never by hand with a fixed number of hours: `local_to_utc(ts, 'America/New_York')` reads a wall clock in its IANA zone, summer time included, and gives the UTC instant; `utc_to_local(ts, 'Europe/Paris')` does the reverse. A source shifted by a fixed offset all year is fixed with both: `local_to_utc(CAST(ts AS TIMESTAMP) - INTERVAL '5 hours', 'America/New_York')`. An hour that happens twice in autumn is read as the first one.

The sources a pack brings (company registers, legal announcements, training catalogues…) are pipelines too, run before their import. They ask the source once per object the organization holds: only the accounts followed, or only the companies linked to a certification of the pack's setting, depending on the source; with that setting empty, every linked company is asked about. A source answering page by page is read until an empty page. Each answer is kept whole in the import's table, and the import reads its nested fields: what a run asked for shows in the import's run history, step by step. Very large files (a day of aircraft positions, published in parts of 2 GB) are read as they arrive and never stored whole: only the positions reach the table, and only for an organization whose storage quota a platform administrator set.

A pack can also bring **named queries**: read-only questions with typed values (two days, an instrument), run like any analysis query of the person asking, with the same rights and limits. A value is checked against its type and can never change the query (*field-invalid* on `params.<name>`); a pack the organization did not install has none (*named-query-not-found*).

## Connectors
Organization admins save access to an outside PostgreSQL or MySQL database; the password stays in the vault. A MySQL connector always uses TLS. **Test** tries to connect. An import reads it with a `connector_query` step, always read only. A private address is refused (*connector-host-refused*) until the platform admin allows that host.

A connector can also point to a **REST API**: a host, a base path and how it authenticates (a bearer token, a key in a header, a user and password, or an OAuth2 client). The import then names the path, where the records sit in the JSON answer, the columns to keep, and how to page (page number, offset, cursor, or the `Link` header). It only reads, page after page, and never follows a next page to another host.

An import can read **only what is new** with a cursor: a column (or a field of the API's records) that grows, such as a modification date. The first run reads everything from the start value; each next run reads past the highest value seen and adds those rows to the table, so a changed record updates its object. A run that finds nothing new adds no version. Deletions at the source are not seen, unless the import reads everything again every so many runs (`full_every`).

A **webhook** connector receives the calls another system sends to its own address. **Generate a secret** shows it once: give it to the sender, which signs each call with it (or paste in PneX the secret the sender imposes). Unsigned or wrongly signed calls are refused. Each call waits up to 7 days in the webhook; an import that names the connector reads the new calls into its table, a later call of the same record updating its object. Declare the columns of the import when the calls do not all carry the same fields.

## Troubleshooting
- ***connector-rest-denied*.** The API refused the credentials: save the connector again with the right token, key or client secret, then **Test**.
- ***connector-rest-no-records*.** The JSON answer holds no list where the import looks: fix the records path of the import.
- ***connector-rest-page-limit*.** More pages than the import allows: raise its page limit or narrow its parameters.
- ***hook-signature-invalid*.** The sender signs with another secret, or with the old one after **Generate a secret**: give it the current secret.
- ***hook-inbox-full*.** Calls pile up unread: schedule the import that reads the webhook.
- ***connector-rest-host-changed*.** The API sent a next page on another host; the import stopped and kept nothing from that run.
- **A run stays queued, then ends in *build-timeout*.** No pipeline worker is running on this server: ask the platform admin.
- ***pipeline-input-missing* or *pipeline-input-empty*.** A table read does not exist or has no version: load it or run what fills it.
- ***pipeline-sql-invalid*.** The query was refused: it can only read, and only the tables listed in the transform, under the names given there. To keep a personal value out of a computed table while still joining on it, write `pseudonym(column)`: the same value gives the same stand-in within the organization, and the real value never enters the new table.
- ***dataset-unchanged*.** The file is identical to the current version: nothing new was kept.
- ***dataset-schema-mismatch*.** The columns of the file differ from those of the table: create another table, or fix the file.
