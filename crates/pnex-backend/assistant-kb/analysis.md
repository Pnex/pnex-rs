---
id: analysis
title: Read-only SQL queries
kind: feature
pages: /pipelines
nodes:
err_codes: analysis-busy, analysis-not-found, analysis-table-unknown, analysis-sql-invalid, analysis-timeout, analysis-result-too-large, analysis-too-many-objects, analysis-unavailable, marking-required
tags: sql, query, requête, analysis, analyse, select, table, objects
---
A member can ask a question in SQL: one read-only `SELECT` on the tables of the organization, and on the objects of a type written `objects.<type>` (for example `SELECT title FROM objects.client`). The objects are read as the member sees them: masked properties stay empty and objects behind a marking they are not cleared for are absent. A marked table needs a clearance.

A query waits its turn on the pipeline worker, before any pipeline; it runs 60 seconds at most and returns its first 1,000 rows. One query at a time per person; the result is kept a day and only its author reads it.

## Troubleshooting
- ***analysis-table-unknown*.** The query names a table that does not exist in the organization, or an object type that does not: check the names on the Tables tab and in the ontology.
- ***marking-required*.** A table of the query carries a marking: ask an administrator for a clearance.
- ***analysis-timeout* or *analysis-result-too-large*.** Filter more, select fewer columns, or compute the result in a pipeline.
