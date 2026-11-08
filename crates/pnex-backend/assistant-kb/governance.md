---
id: governance
title: Markings and clearances
kind: feature
pages: /pipelines/tables/:id
nodes:
err_codes: marking-required, marking-not-found, marking-key-taken, marking-in-use, clearance-not-found
tags: marking, marquage, clearance, habilitation, purpose, finalité, declassify, déclassifier, sensitive, sensible, governance, gouvernance
---
A **marking** is a compartment of the organization, such as HR or Health. A table that carries markings shows its rows only to people **cleared** for every one of them; its name, columns and versions stay visible to all. The owner and the administrators need a clearance too.

An administrator creates the markings, marks a table, and clears a member for a marking: a clearance names a **purpose** (why this person reads this data) and ends after 1 to 365 days, or when it is revoked. Every read of marked data, every clearance and every change of markings is kept in a journal, with the purpose.

What a pipeline computes from a marked table carries the same markings. Removing a marking from a table is a **declassification**: it needs a written reason, kept in the journal, and a later run that still reads marked data puts the marking back.

## Troubleshooting
- ***marking-required* on a table page.** The table carries a marking you are not cleared for, or your clearance ended: ask an administrator for a clearance, with the purpose.
- ***marking-in-use* when deleting a marking.** A table still carries it: declassify that table first.
