---
id: market-moves
title: Market moves and world events — abnormal sessions, same window, attribution
kind: feature
pages: /ontology
nodes:
err_codes: ontology-link-member-only, import-key-required
tools: market_session, market_rule_test
tags: market move, mouvement de marché, abnormal session, séance anormale, crash, krach, sell-off, index, indice, Nasdaq, S&P 500, VIX, oil, pétrole, WTI, Brent, rates, taux, dollar, sigma, world event, événement du monde, geopolitical, géopolitique, war, guerre, conflict, conflit, attack, attaque, sanctions, blockade, blocus, ceasefire, cessez-le-feu, GDELT, UCDP, ACLED, verified, vérifié, map, carte, concomitance, same window, même fenêtre, cause, attribution, attributed to, attribué à
---
PNEX keeps two kinds of dated facts next to company filings: the **abnormal sessions** of a few market series, found on the series themselves, and **world events** (conflicts, attacks, sanctions…), each with a place, a date and a source. It never says that one caused the other. It is an aid to analysis, not investment advice.

## Abnormal sessions
With the **Markets** pack, two settings list the series to watch:
- **FRED series watched for abnormal sessions**: by default the VIX, the US 10-year rate, WTI and Brent oil and the trade-weighted dollar. FRED's S&P 500 and Nasdaq series are not in it: their owners require a permission before any use.
- **Tickers watched for abnormal sessions (Tiingo)**: by default SPY (S&P 500) and QQQ (Nasdaq-100), read with your own Tiingo key (secret `markets_tiingo_key`), by the import **Watched prices (Tiingo)**.

Each entry reads `CODE:mode:class`:
- mode `log_return` for prices and indices, `diff` for rates and the VIX;
- class `equity_index`, `commodity`, `rates`, `volatility`, `fx` or `crypto`;
- optionally `:k:floor` to change the thresholds of one series.

The imports **Watched series (FRED)** and **Watched prices (Tiingo)** read them every day. A session counts as abnormal when its change is at least 3 times the series' usual volatility over the year before (sigma) **and** at least a floor: 2 % for a price, 0.75 % for a currency, 0.15 for a rate or the VIX. Each one becomes a **Market event** of kind *Series move*, linked to its series. The org gets one notice per session listing its moves.

To also get it by e-mail, Slack, ntfy or any other channel of the organization, write the channel names in the setting **Channels of the abnormal session notice**, separated by commas.

The first reading of a series finds its whole history at once: those events are marked **Caught up from history**. They are not announced, and tests that read the past leave them out, because their real publication date is lost.

Sources to cite when you show these figures: the VIX belongs to the Chicago Board Options Exchange; oil prices come from the U.S. Energy Information Administration; the dollar index comes from the Federal Reserve Board.

## The Session tab
A market event and a world event have a **Session** tab. It lists the abnormal moves of the window, one session before and after, the biggest first. Under them it shows the world events of the same window, dismissed ones left out. **Same window does not mean cause.**

## World events
The **World events** pack reads the GDELT feed for the countries in the setting **Countries watched**, as ISO codes: UA for Ukraine, RU for Russia, IL for Israel, IR for Iran. The setting is empty by default, so nothing is read until you fill it. The codes kept are in **Kinds of events kept (CAMEO codes)**.

The feed is noisy, so PNEX makes one event per country, kind and day, with the number of sources and articles. It always starts as **Unconfirmed**: it feeds no test by default.

A lead gets confirmed in two ways:
- **By a verified event**: the UCDP and ACLED imports bring events coded by hand by these programs, already confirmed, one per country, kind and day (with the total deaths and sources). A GDELT lead of the same country, at most one day apart and of a compatible kind, then turns *Confirmed*, and **Confirmed by the verified event** gives the reference.
  - UCDP is free (CC BY 4.0). Its downloads need nothing: run **Verified events history (UCDP GED download)** once to get the history since 1989, and **Verified events this year (UCDP Candidate download)** follows the current year every month. Each day is dated by the first article that reported it, so the history can be studied without reading the future; a day without a report date stays out of those tests. The address of these imports names UCDP's release: update it when UCDP publishes a new one.
  - For a live reading, **Verified events (UCDP)** uses UCDP's API, which needs a token: ask Uppsala University for it by e-mail, then store it in **Secrets** under the name `world_events_ucdp_token`.
  - ACLED is read only with your organization's own ACLED account and licence: store the e-mail and the password in **Secrets** under `world_events_acled_email` and `world_events_acled_password`. PNEX provides no ACLED data, and a commercial use needs ACLED's commercial licence.
- **By hand**: set the status to *Confirmed* and fill **Confirming source**.

A world event shows on the general **Map** when **World events (30 days)** is ticked in its side panel, green when confirmed and amber when not. It also appears on the map of a case it is attached to.

## Attributing a move
Only a member can say that a move is attributed to a world event or an article, and only in a case: install the **Market attribution** pack, attach the market event to the case, then add the link **is attributed to** from the market event, with a justification and a source. Both are required.

## Testing a reaction
On an instrument or a macroeconomic series, the **Reaction** tab measures how this series moved after world events or after abnormal sessions, next to dates drawn at random:
- each event is dated by when it became known, not by the day it is said to have happened;
- events closer than about 20 sessions to an earlier one count once, and the number merged is shown;
- unconfirmed world events stay out unless you include them, and their share is then shown.

## Assistant
The assistant lists a session with its world events (`market_session`) and runs these tests (`market_rule_test`). It may point out that an event is in the same window, never that it caused a move. It never writes **is attributed to**.

## Troubleshooting
- *This link is written by a member, from a case that holds its source* (`ontology-link-member-only`): the market event is not in a case. Attach it to the case first, from the case, then add the link.
- **No world event appears**: the setting **Countries watched** is empty or holds a code that is not ISO (UK instead of GB, for example). Fix it, then run **World events (GDELT)** again.
- *This source takes an API key* (`import-key-required`) on **Verified events**: the token (UCDP) or the account (ACLED) is not in **Secrets** under the expected names.
- **The UCDP import fails after months of working**: UCDP published a new release and dropped the old one. Put the new version in the address of the import.
- **No abnormal session after a big day**: FRED publishes some series a day or more later, oil a few days later. The move appears the day its value is read.
