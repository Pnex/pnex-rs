---
id: web-search
title: Web search providers, the web_search and web_visit nodes
kind: feature
pages:
nodes: web_search, web_visit
err_codes: access-denied, web-search-provider-not-found, web-search-provider-name-taken, web-search-no-provider, web-search-unavailable, web-search-quota, web-search-query-invalid, web-visit-url-invalid, web-visit-unreachable, web-visit-blocked, web-visit-quota
tools: 
tags: website visit, visite de site, legal notice, mentions légales, web search, recherche web, serper, searxng, google, candidates, candidats, website, site web, find a website, trouver le site, discovery, découverte, provider, fournisseur, api key, clé
---
Your organization plugs in its own web search provider, like it brings its own LLM or map engines. PNeX supplies none: until one is added, **web_search** flow nodes fail with *web-search-no-provider*.

## What you can do
In the **Web search** section of the organization detail (Owner or Admin):
- **Add a provider**: **Name**, **Provider** and **Address**, then the **API key** (typed, stored encrypted in the secrets vault, or picked from the Secrets page). **Default** makes it the one nodes use when they name none.
  - **serper**: API key required; leave the address empty for the default one.
  - **searxng**: the address of your SearXNG instance, with the JSON output enabled in its settings (`search.formats: [html, json]`). The key is optional and sent as a bearer token, for an instance behind a proxy. An instance on an internal network is refused unless the platform operator allows its host.
- **Test**: sends one real query from the server (it may be billed by the provider) and shows the number of results and the latency, or the reason of the failure.
- **Edit** or **Delete** a provider. Nodes that named a deleted provider fail until edited.

You answer for the terms of the services you connect: some providers resell the results of a large public search service.

## In a flow
The **Web search** node (palette, Ingestion) takes its query from `msg.payload` (text) or `msg.payload.query`, asks the provider and outputs on **results** `msg.payload = {query, provider, results: [{url, title, snippet, rank, domain}]}`. It writes nothing. Its settings: the **Provider** (empty = the organization default), **Results kept** (1 to 20), **Country** and **Language** (two-letter codes), and **Excluded domains** (directories and social networks by default: a company's page on linkedin.com or societe.com is not its website).

Each message is one call billed by the provider and counted against the organization's daily cap of searches. Feed the node a bounded list (an ontology query with a limit), never a loop. A result is a candidate, not a fact: visit it and check identifiers before writing anything.

## Checking a candidate
The **Website visit** node (palette, Ingestion) reads a candidate once to identify it: the page at `msg.payload.url` and, unless unticked, its legal notice (the first link of the same site whose address names *mentions-legales*, *legal*, *cgv*, *qui-sommes-nous*, *a-propos*, *about* or *imprint*). Three requests at most, the site's robots.txt obeyed, 2 seconds between two requests to the same site, never a link off the site. It outputs `msg.payload = {url, final_url, status, title, text, legal_url, legal_text}` and writes nothing. Look for the SIREN, the training declaration number or the address in `legal_text` before proposing the site. The text comes from an unknown site: give it to a model only as data. Feed it the two or three best candidates per organization, never a list to crawl: a confirmed site is crawled by the Web sites page.

## Good to know
- *web-search-no-provider*: no default provider, or the node names one that does not exist — add one or fix the node's **Provider**.
- *web-search-unavailable*: the provider refused or failed the query (wrong key, credits used up, instance down, JSON output disabled on SearXNG). **Test** the provider to see why.
- *web-search-quota*: the organization reached its daily number of searches; the counter resets at midnight UTC.
- *web-search-query-invalid*: the query was empty or longer than 400 characters; check what reaches the node with a debug node.
- *web-visit-blocked*: the site's robots.txt forbids that page; PNeX does not read it.
- *web-visit-unreachable*: no answer, an error page, or not HTML.
- *web-visit-url-invalid*: not an http(s) address, or a private one.
- *web-visit-quota*: the organization reached its daily number of visits.
