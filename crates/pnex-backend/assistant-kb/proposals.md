---
id: proposals
title: Proposals to validate
kind: feature
pages: /proposals
nodes: 
err_codes: proposal-decided, ontology-version-conflict
tools: 
tags: to validate, à valider, proposal, proposition, accept, accepter, reject, rejeter, inbox, review, machine, community data, wikidata, candidate, candidat, decided by, décidé par
---
Some writes come from a source that is not sure it is right: a group link from a community source, a website candidate, a value read by a model. They do not enter the ontology on their own. They wait as **proposals** on the **To validate** page (Knowledge menu), and on the page of each object concerned.

## What you can do
- See what is proposed: what (new object, change, new link, link changed or closed), the objects concerned, the proposed values, the origin (an import, a flow, a detector) and its confidence.
- **Accept** (members): the value is written in the ontology with the same checks as an edit by hand, and its history says it came from a proposal.
- **Reject**: the proposal is kept with its reason and the same value is never proposed again; another value would be a new proposal.
- Filter by status: to validate, accepted, rejected, replaced (a newer proposal for the same thing replaced it before anyone decided).

Viewers read the proposals but do not decide. The assistant never decides: it can only list them.

## Good to know
- *proposal-decided*: someone already decided it the other way, or a newer proposal replaced it; refresh the list.
- *ontology-version-conflict* on accepting a change: the object was edited since the value was read; the proposal stays to validate; reject it if it no longer applies.
