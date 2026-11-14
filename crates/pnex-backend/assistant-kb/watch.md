---
id: watch
title: Watch: news items, news sources and shows
kind: feature
pages:
nodes: ontology_query, object_upsert, link_upsert, llm_text
err_codes:
tools: 
tags: watch, veille, news, actualité, news item, press, presse, RSS, Atom, feed, flux, newsroom, source, show, salon, trade show, exhibitor, exposant, organizer, organisateur, groupement
---
Install the **Watch** pack to follow news and shows. It brings three types and ships no content: no topic, source, show or list of companies. What you follow is yours.

- A **News item** holds a key (its address, or the key an import gives it), a summary, a quoted excerpt, a date, its source, a free-text topic and a state: *To validate*, *Kept* or *Dismissed*. A flow may dismiss an item about something else; only a person keeps one.
- A **News source** is an address (an RSS or Atom feed, or a page). Link it to its company with **is a source of**: the items it brings (**comes from**) are that company's own news. The import **News feeds (RSS, Atom)** reads every source of format *RSS or Atom feed* and makes one news item per article, never twice; give it a recurrence (once a day is enough). Only the addresses you entered are read, without any key or password.
- A **Show** has its dates, venue, city, country and website. Link the companies that exhibit with **exhibits at** (with their stand), and the body that runs it with **is organized by**: a show of a trade association is just a show organized by that association.

Linking news to companies is done in your flows: a regex or an AI node decides which company an item is about and its topic, then **Link upsert** writes **is about**. When the match is sure (a SIREN, the exact name), link directly; when it is not, set the link to proposed so a person validates it on **To validate**. The same goes for exhibitors read from a show's exhibitor list.

The import **Press (GDELT)** of the **B2B prospecting, France layer** pack writes news items about the accounts you follow, and the flow model **Prospecting · news to topics** gives them a topic. In **Sales follow-up**, a trade show is a show: name a list on it to turn the list's contacts into leads.

To read a company's news, open it: its news items and the shows it exhibits at are in its links.
