---
id: company-groups
title: Company groups, target lists, target score and finding a company
kind: feature
pages:
nodes: org_resolve
err_codes: org-resolve-unavailable, org-resolve-quota, org-resolve-query-invalid, import-schema-changed
tools: 
tags: meeting, rendez-vous, debrief, compte rendu, playbook, coaching, participant, recording, enregistrement, account plan, plan de compte, competitor sheet, fiche concurrent, meeting preparation, préparation de rendez-vous, commercial signal, signal commercial, BODACC, legal announcement, annonce légale, digest, résumé, white space, espace blanc, coverage, couverture, colour, couleur, status, statut, target list, liste de ciblage, ideal customer, client idéal, ICP, target score, score de ciblage, criterion, critère, group, groupe, subsidiary, filiale, parent company, société mère, holding, GLEIF, LEI, Wikidata, SIREN, find a company, retrouver une société, company name, nom de société, trade show, salon, account mapping, cartographie commerciale
---
A company's group is built from two public sources, side by side, never merged. Install the **French companies** pack (and **B2B account mapping** for account statuses): it adds three imports next to the company register one, on the **Imports** tab of the ontology.

## Group links
- **Legal entity identifiers (GLEIF)**: for each known SIREN, the LEI and the parents the company declares (direct and ultimate). The link **is a subsidiary of** gets the nature *direct_consolidation* or *ultimate_consolidation*. When the direct and the ultimate parent are the same company, one link is kept.
- **Parent companies (GLEIF)**: creates the parents GLEIF names. A French parent is created by its SIREN, a foreign one by its LEI with its country. The next run of the first import then links the subsidiary to it.
- **Parents and subsidiaries (Wikidata)**: parents and subsidiaries that have a SIREN in Wikidata. Each link (nature *wikidata*) is a **proposal** on the **To validate** page: it is community data (a state agency can show up as the parent of a listed company). Accept it and it enters the group; reject it and it is never proposed again.

Each import takes the known companies in turn, up to its page budget per run: a large list is covered over several runs. Run them with **Run now**, or set a recurrence. A run that finds nothing for its batch is normal.

Large groups are better covered by Wikidata, mid-sized ones by GLEIF; a company in neither gets its parent by hand. See the tree on the object page, hierarchy **Commercial group** (subsidiaries, holdings, then establishments).

## Coverage and colours
Account and site statuses carry a colour: customer green, prospect amber, target blue, competitor red, partner violet, the others gray. The colour shows next to each node of the hierarchy, around the node in the object's graph, and around a site's pin on the map.

Above the **Commercial group** tree, a band counts the statuses of the group (each company or site once, even under two parents) and sums **Current users** and **Potential users**. A node with neither a contact (**employs**) nor a customer status is **white space**: tick **White space only** to list them, that is where to prospect next in the account.

## Commercial signals
A **Commercial signal** is something that moved on an account: its *Detected by* (legal announcement, new establishment, closed establishment, change of officer, page changed…), a *Theme*, a *Summary*, the *Quoted excerpt*, the *Date* and the *Source*, linked to the account by **is about**. Its *State* starts at *to_validate*; set it to *retained* or *dismissed* once read, an import never resets it.

The **B2B prospecting, France layer** pack adds the import **Legal announcements (BODACC)**: for each account whose status is customer, prospect or target, the announcements of the register of commerce (changes, sales, openings, insolvency proceedings, registrations; account filings are left out) become signals. A *Créations* announcement is a new establishment. Run it with **Run now** or give it a recurrence (weekly is enough).

The flow model **Prospecting · signals digest** makes, each morning, one text of the signals not yet notified (`payload.text`, `payload.count`) and marks them notified. Add your **Notification** node on its first port (a template with a `text` variable, trigger `payload.trigger`), then deploy. The digest goes to the channels you choose; sending to each account's followers is not done yet.

## Account plan, competitor sheet, meeting preparation
On an organization's page, **New page** offers the page templates of the installed packs made for organizations: **Account plan** (the account, its group, coverage, white space, contacts, recommendations, followers, signals, solutions in place, next actions), **Competitor sheet** (the accounts known to use it, its signals, defence arguments) and **Meeting preparation** (the account in brief, coverage, known contacts, who can introduce you, recent signals, competition, questions by persona). The page is filled with what PneX knows today, linked to the organization (**is a page about**) and opened in the page editor; it is a snapshot you complete freely, not a live view: create a new one to refresh it.

## Meetings and coaching
A **Meeting** (rendez-vous) is an object: date, purpose, status (planned, held, cancelled), place, next step. Link it to the account (**is a meeting with**), its participants (**takes part in**, side *internal* or *customer*), its recording (a media: **is the recording of**) and the **Playbook criteria** it covered (**covered**, with a comment). The criteria are your own list (objects *Playbook criterion*, with the persona they target).

On the meeting's page, **New page** › **Meeting debrief** fills who was there, the account, the recording and the criteria covered, with sections for the summary, objections and next steps. Transcribe the recording and summarize it from the media's Analyses card, with *What the summary must bring out* set to what you need; paste what you keep into the debrief once reread.

Coaching is read per criterion, not per person: on a criterion's page, hierarchy **Coaching** lists the meetings that covered it and their accounts, the band counts them by status.

## Finding a company from its name
The **Find a company** node (palette, Ingestion) looks up a name in the public company register and scores up to five candidates: name likeness 70 points, same postcode 20 or same city 15, still active 10. Input `msg.payload = {name, city, postcode}`, or the name alone (then 80 points at most). Outputs:
- **matched**: the best candidate reaches the threshold (80 by default) and leads the next one by the gap (10); `msg.payload.siren` is set.
- **to review**: ambiguous or nothing found; send it to a list a person checks.
- **failed**: `msg.error`.

It writes nothing. Treat a match as a proposal: write what follows as to be validated.

## Target lists
The **B2B prospecting, France layer** pack adds the import **Target list (company register search)**. Its address is a search of the public company register with its filters: activity (`activite_principale` NAF codes or `section_activite_principale`), headcount bands (`tranche_effectif_salarie`), `departement`, `etat_administratif=A`. The shipped address is an example (manufacturing, 100 employees and more): edit it to your ideal customer before **Run now**. The register serves 10,000 results at most per search: split a larger one (by département, by band).

New companies enter with the account status **target**; a company you already know keeps its status (a client stays a client). The search brings the company and its head office; the company register import brings its other sites, company by company.

## Target score
The score of an organization is the sum of the points of the **Targeting criteria** it meets. Each criterion is an object you create: **Property** (a property key such as `activity_code`, `headcount_min`, `last_revenue`, `country`, `account_status`), **Operator** (*equals*, *one_of* and *starts_with* take a comma-separated list, *at_least*, *at_most*, *filled*), **Value**, **Points** (negative to push down, e.g. existing clients) and **Active**. The flow model **Prospecting · target score** writes *Target score*, *Score detail* (one line per criterion met) and *Scored on*: deploy it once your criteria exist. It scores up to 500 organizations not yet scored today every 15 minutes. Sites are not scored: filter them on their own headcount in the explorer.

## Trade show and association lists
The **B2B account mapping** pack adds the import **Trade show or association list (file)**: a CSV with the columns `list`, `name`, `job_title`, `company`, `city`, `postal_code`, `email`. Each row becomes a **List contact**. With the **B2B prospecting, France layer** pack, the flow model **Prospecting · list contacts to companies** looks each company up in the public register (**Find a company**): a match creates or finds the organization by its SIREN and proposes the link **is a contact at** on the **To validate** page; an ambiguous name marks the contact *to_review* with the candidates, nothing found marks it *not_found*. Check the *to_review* contacts by hand.

## Watched pages
A **Watched page** is a news or careers page you follow (address, the account it belongs to). The site crawl stores its text and marks it *changed* when the text differs from the last visit. The flow model **Prospecting · watched pages to signals** reads the changed pages every hour, asks your organization's model whether the change is about a topic that matters (new site, hiring, investment, management change, new product, partnership) and, if so, writes a **Commercial signal** *page_changed* to triage on the account. It needs an AI provider in Settings.

## Who can introduce me
On an object's page, **Who can introduce me? Starting from** a link type (for instance the one that says who follows an account) shows the shortest chain of links between people (knows, recommends, employs, manages, member of), at most 4 links, from anyone at the source of that link type to this object. "No path" means nobody in reach: add who knows whom.

## Good to know
- *org-resolve-unavailable*: the register did not answer; try later.
- *org-resolve-quota*: the organization reached its daily number of lookups; the counter resets at midnight UTC.
- *org-resolve-query-invalid*: empty name, or longer than 255 characters.
- *import-schema-changed* on a group import: the source changed its format; the run wrote nothing.
