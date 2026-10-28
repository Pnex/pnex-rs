---
id: sales-follow-up
title: Sales follow-up — opportunities, activities, follow-up plans, morning digest
kind: feature
pages:
nodes: ontology_query, object_upsert, link_upsert, llm_text
err_codes: prospection-opposed, ontology-link-cardinality
tools:
tags: crm, sales, vente, suivi commercial, opportunity, opportunité, pipeline, stage, étape, qualification, SPICED, MEDDPICC, criterion, critère, activity, activité, call, appel, mail, e-mail, follow-up, relance, plan de relance, draft, brouillon, morning digest, ma journée, dormant, sans activité, forecast, montant, amount, trade show, salon
---
Install the **Sales follow-up** pack from the ontology's packs tab: it installs **B2B account mapping** first, then adds its types, links and five flow models (drafts). Nothing in it is your playbook: stages, criteria and follow-up plans are yours.

## Opportunities
An **Opportunity** is an object: *Stage* (discovery, qualification, proposal, negotiation, won, lost — rename them to your own words in the type's value labels, the keys stay), *Amount*, *Probability (%)*, *Expected close*, *Source* (trade_show, buying_group, inbound, outbound, referral, network, other) and *Source detail* (the trade show's name), *Reason lost*. Link it to its account with **is an opportunity with** (one account per opportunity), the people of the buying committee with **is involved in** (their *Role*), the person who carries it with **is carried by**, and where it came from with **comes from** (a commercial signal or a meeting). A meeting about it is linked with **is a meeting for**.

On an account's page, hierarchy **Opportunities** lists its opportunities; the band counts them by stage. On an opportunity's page, **New page** › **Opportunity sheet** writes a snapshot: account, stage, people, qualification, activities, meetings, then next steps.

## Qualification
Your grid (SPICED, MEDDPICC or your own) is a list of **Qualification criteria**: a title, a *Description*, *Required from stage*, an *Order*, *Active*. On the opportunity, link each criterion with **is qualified on**, *Status* unknown, partial or valid, and a *Note*. The flow model **CRM · missing criteria** writes *Missing criteria*: the active criteria required up to the opportunity's stage that are not valid. It never blocks a stage: you move the stage yourself.

## Activities and follow-up plans
An **Activity** is something done or to do: *Kind* (call, mail, meeting, network_message, note, task), *State* (to_do, done, cancelled), *Due on*, *Done at*, *Direction*, *Content*, linked with **is an activity on** to an opportunity, an account or a person. Set *State* to done (and *Done at*) when it is done: the flow model **CRM · last activity** moves the opportunity's *Last activity* forward. With **CRM · mails received** deployed, a mail from a person becomes a done activity on that person and on their open opportunities, so it moves their *Last activity* too. An *Amount* is a sum with its currency (for instance 48 000 EUR).

Your **follow-up plan** is a list of **Follow-up steps**: *Day* after the start, *Activity* kind, *Instruction*, *Active*. With the flow model **CRM · follow-up plan** deployed, an opportunity that enters the stage qualification gets one activity to do per active step, due on its day; *Follow-up started on* records it. An opportunity whose people involved all objected to prospecting gets no activity.

The flow model **CRM · drafts** writes a *Draft* on the mails and network messages due by tomorrow, with your organization's LLM, from the step's instruction and the account. **Nothing is ever sent by PneX**: read the draft, send it yourself from your mailbox or the network, then set the activity to done.

## My day
The flow model **CRM · my day** makes one text each morning: the activities due today or late, and the open opportunities with no activity for 14 days. Add your **Notification** node on its first port (template with a `text` variable, trigger `payload.trigger`), then deploy. To change the delay, edit `$today-14` in its query node.

## When a link is refused
**prospection-opposed** on a link: the person objected to the processing of their data, or was erased, and this link would recreate them at the same account. Do not insist: the objection is kept on purpose. See the card on personal data.
