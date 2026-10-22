---
id: proposals
title: Proposals to validate
kind: feature
pages: /proposals
nodes: 
err_codes: proposal-decided, ontology-version-conflict
tools: 
tags: voice, voix, recognized voice, voix reconnue, automatic, automatique, rule, règle, to validate, à valider, proposal, proposition, accept, accepter, reject, rejeter, inbox, review, machine, community data, wikidata, candidate, candidat, decided by, décidé par
---
Some writes come from a source that is not sure it is right: a group link from a community source, a website candidate, a value read by a model, a detection on the tracks of an investigation case. They do not enter the ontology on their own. They wait as **proposals** on the **To validate** page (Knowledge menu), and on the page of each object concerned.

## What you can do
- See what is proposed: what (new object, change, new link, link changed or closed), the objects concerned, the proposed values, the origin (an import, a flow, a detector) and its confidence.
- **Accept** (members): the value is written in the ontology with the same checks as an edit by hand, and its history says it came from a proposal.
- **Correct and accept** (proposals that carry text values): fix a value before it is written; the decision keeps the correction.
- **Reject**: give a reason if you wish; the proposal is kept with it and the same value is never proposed again; another value would be a new proposal.
- **Automatic** (in the Decided by column): a proposal accepted by a rule of the organization, without anyone deciding, for example a website whose score reaches the confirming score of the Web presence pack (Ontology › Packs › Settings). The rule and the score are shown; if it is wrong, close the link on the object page: the flow never opens it again.
- **Case event**: a detection of an investigation case (two members together, a stop, a signal lost); accepting it adds the event to the case, as **Retain** does in the case; *Correct and accept* edits the event's title.
- **Recognized voice**: a person recognized by their voice on an audio or video file (consented voice profile). It is listed here but decided on the file's review page, where you can hear it and move its span: **Listen and decide** opens it.
- Filter by status: to validate, accepted, rejected, replaced (a newer proposal for the same thing replaced it before anyone decided).

Viewers read the proposals but do not decide. The assistant never decides: it can only list them.

## Good to know
- *proposal-decided*: someone already decided it the other way, or a newer proposal replaced it; refresh the list.
- *ontology-version-conflict* on accepting a change: the object was edited since the value was read; the proposal stays to validate; reject it if it no longer applies.
