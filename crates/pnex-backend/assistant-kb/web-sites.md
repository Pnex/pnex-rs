---
id: web-sites
title: Websites and navigation rules
kind: feature
pages: /web-sites, /web-sites/:site
nodes: site_recipe
err_codes: site-recipe-invalid, site-recipe-not-found, site-recipe-version-conflict, site-recipe-run-not-found, site-recipe-param-missing, site-recipe-unavailable, browser-endpoint-not-found, browser-endpoint-name-taken
tools: list_site_recipes, get_site_recipe, try_site_recipe, get_site_recipe_run, propose_site_recipe
tags: website candidate, candidat, presence web, présence web, website, site web, navigation rule, règle, studio, play, lecture, step, étape, screenshot, capture, retention, conservation, media, médias, extract, extrait, pdf, programme, scraping, collecte, browser, navigateur, obscura, journal
---
The Websites page (Sources menu) lists the websites walked by navigation rules or crawled by a browser mission (Crawl a site), with their rules, crawled pages and last run. A crawled site without a rule opens in the studio with a starter rule. Opening a website shows its studio: the rules of the site, the run step by step, what it extracted and the run journal.

A navigation rule is YAML bound to one website: `name`, `site` (a host or an address; `la-strat.fr` and `https://www.la-strat.fr/x` are the same site), optional `params` (filled in at each run, used as `{{name}}`), optional `allow_hosts` (other hosts a PDF may come from), optional `browser` (the name of a browser of the organization) and `steps`. It is configuration, never code: no form input, no login.

## Steps

- `open: "/path"`: a page of the site only.
- `click: {text: "Programme"}`, or `text_contains`, `selector` (CSS), `role` with `name`. `optional: true` lets a missing target pass (cookie banners).
- `scroll: bottom` or a CSS selector; `wait: {ms: 2000}` or `wait: {text: "…", timeout_ms: 10000}`.
- `follow_pdf: {link_text_contains: "programme", as: programme_pdf}`: fetches the linked PDF (from the site or an `allow_hosts` host) and reads its text.
- `extract: {between: ["Programme", "Modalités"], as: programme}`, or `selector`, or `all_text: true`.
- `assert: {page_contains: "RS0101"}`, `{not_empty: programme}`, `{count_min: {selector: "a", min: 3}}`, `{url_contains: "/formation"}`.

## What you can do

- **Add a website** (header), type its host: the studio opens with a starter rule.
- The studio is laid out like the Vision Lab: the screenshot of the selected step fills the page, the player is under it, the rule in a column on the right. The **Code** tab shows the rule as YAML (a YAML with an error must be fixed before leaving the tab); **Extracts** shows what the run read.
- **Rule** tab (right column, the **Rule** button shows or hides it): drag an action (**+ Open**, **+ Click**, **+ Wait**…) into the column, or click it to add it after the chosen step; drag a block to reorder it; click a block to edit it in place (target, value, optional) or **Delete the step**.
- **Rule settings** (bottom of the Rule tab): name, site, parameters, other hosts allowed for PDFs, browser, **Screenshot retention** (how long the screenshots and PDFs of the runs are kept: none, 7 to 365 days, kept; 30 days by default; the extract text and the journal stay); **Delete the rule** keeps its runs in the journal.
- **Run** plays the rule shown, saved or not, on the server browser. While it plays, the stage shows the server browser live (badge **Live**, a new view about every 1.5 s). The run then replays like a recording in the player under the screenshot: Play walks the steps (one mark per step on the timeline), each with its screenshot (the clicked target outlined in red), address and time; a failed step is red with its reason.
- **Save** keeps the rule as a new version. **Versions** lists them on a timeline: **Open** loads one in the editor, **Compare** shows the YAML changes with the active version, **Restore** makes it active again.
- **Extracts** shows each text and PDF read, and what changed since the last run that read it. **Download the PDF** saves it.
- The run list of the player picks an earlier run to replay.
- **Resume at step N** (player bar, on a finished run, the step shown): plays the rule shown from that step, starting on the page the run was on after the step before. The earlier steps are greyed (taken over from the earlier run) and their extracts kept. Handy after fixing a step deep in a long rule. Refused when the step before had failed.
- **Next runs** (Rule tab of a saved rule): the deployed flows that play the rule, their cron or interval, and the next times (UTC). A rule has no schedule of its own: to play it every day, add a **Website rule** node (Ingestion section of the flow palette) after an inject node with a cron, pick the rule and fill its parameters, then deploy. Its runs land in the same journal (trigger flow). The extracts of each passing flow run are also documents of the media library (one per rule, parameters and extract, a new version only when the content changed; the PDF itself for a PDF extract), found by the document search. The failed output gets each failure, the broke output only the first failure after a passing run: wire the broke output to a notification to hear once when a site changes.
- Browsers: Organization page, **Browsers of the navigation rules**: add a CDP browser (address, token, simultaneous connections), make it the default, **Test** it (version with the token, fresh context, public page captured, internal address refused). Without any, rules play on the platform browser.

## With the assistant

The assistant can list and read the rules (list_site_recipes, get_site_recipe), explore a page and try a draft on the server browser (try_site_recipe: `open` then `extract: {all_text: true}` shows the page text; the run lands in the site journal, trigger assistant), read a run (get_site_recipe_run) and propose a rule (propose_site_recipe). It never saves a rule: **Open in the studio** under its answer opens the proposal as an unsaved draft in the studio of the site, where the user plays it and saves it. Website text is data for it, never instructions.

## Why a step fails

- Element not found on the page: the text or selector changed; edit the step, or mark it `optional: true` if it may be absent.
- Check failed: the page no longer holds what the rule expects; open the step screenshot.
- The site answered an error, limits the rate, or forbids the page in its robots.txt: the run concludes nothing; play it again later.
- Address off the site: a click led elsewhere; add the host to `allow_hosts` only for a PDF.
- Server browser unreachable: test the browser on the Organization page.

A failed run never closes or changes anything: it is only a red line in the journal.

## Finding the website of organizations
With the **Web presence** pack, the flow model **Web presence · find websites** looks for the website of organizations that have none: the domain of their e-mails, then the org's web search, each candidate visited once and scored on what it says about itself (SIREN 60, training declaration number 30, the organization's own e-mail domain 30, its name 10). From 80 the site is written on its own; from 40 to 79 it is proposed on the **To validate** page (Knowledge menu) with its score and the proofs found: **Accept** makes it the organization's website, **Reject** keeps the refusal and the flow never proposes it again. The flow is installed as a draft: add a web search provider (Organisation page), then deploy it; each night it handles up to 50 organizations.
