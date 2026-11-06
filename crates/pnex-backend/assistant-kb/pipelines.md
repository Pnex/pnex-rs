---
id: pipelines
title: Pipelines, tables and connectors
kind: feature
pages: /pipelines, /pipelines/:id, /pipelines/tables/:id
nodes: dataset_trigger
err_codes: pipeline-not-found, pipeline-name-taken, pipeline-version-conflict, pipeline-document-invalid, pipeline-run-active, pipeline-input-missing, pipeline-input-empty, pipeline-sql-invalid, pipeline-memory-exceeded, pipeline-timeout, pipeline-worker-lost, pipeline-check-failed, build-not-found, build-cycle, build-too-long, build-author-forbidden, build-timeout, dataset-name-taken, dataset-reserved, dataset-csv-invalid, dataset-schema-mismatch, dataset-quota, dataset-source-too-large, dataset-unchanged, connector-name-taken, connector-unreachable, connector-host-refused
tags: pipeline, table, dataset, transform, traitement, sql, lineage, lignage, version, retention, rétention, check, contrôle, freshness, fraîcheur, schedule, planification, chained run, exécution enchaînée, connector, connecteur, csv
---
The Pipelines page (Sources › Pipelines) holds versioned **tables**, the **pipelines** that compute them in SQL, the **chained runs** and the **connectors**.

## Tables
A table keeps its past versions. Create one and load a CSV from its page (5 GB at most, sent in parts, then converted in the background): the file becomes a new version, and readers see it only once it is complete. The page shows the first rows with how full each column is, each version with its checks (**Read this version** shows the rows as they were), the lineage (what the table reads and what reads it) and the expected freshness: past that many hours without a new version, the organization is warned. A table filled by an import belongs to it, and its page names that import: neither a CSV nor a pipeline can write it (*dataset-reserved*).

## Pipelines
**New pipeline** asks for a name, a table to read and a table to write. The studio shows the tables as a graph, left to right. Click a table to see its first rows; click a table the pipeline writes to edit its **transform**: the tables it reads, each under a name, the SQL query, and its **checks** (minimum and maximum rows, unique columns, share of empty values, fall in rows, same columns). A check that refuses keeps the previous version of the table (*pipeline-check-failed*). **Add a transform** chains a new one after the selected table. The **Code** tab shows the same pipeline as YAML; there, `retain_days:` on a transform sets how many days a replaced version of its table stays readable (1 to 3650, 90 when absent; the latest version always stays). A table loaded from a CSV keeps 90 days. **Versions** opens, compares and restores earlier versions.

**Run** works on the saved version. If the pipeline depends on other pipelines or imports, the page lists what will run first: an API import spends its quota. **Schedule** sets one schedule, on the last table of a chain: what it depends on runs before it, and a failure blocks only what depends on it, which keeps its data. An import whose run was cut by a server restart starts again once by itself.

## Connectors
Organization admins save access to an outside PostgreSQL database; the password stays in the vault. **Test** tries to connect. An import reads it with a `connector_query` step, always read only. A private address is refused (*connector-host-refused*) until the platform admin allows that host.

## Troubleshooting
- **A run stays queued, then ends in *build-timeout*.** No pipeline worker is running on this server: ask the platform admin.
- ***pipeline-input-missing* or *pipeline-input-empty*.** A table read does not exist or has no version: load it or run what fills it.
- ***pipeline-sql-invalid*.** The query was refused: it can only read, and only the tables listed in the transform, under the names given there.
- ***dataset-unchanged*.** The file is identical to the current version: nothing new was kept.
- ***dataset-schema-mismatch*.** The columns of the file differ from those of the table: create another table, or fix the file.
