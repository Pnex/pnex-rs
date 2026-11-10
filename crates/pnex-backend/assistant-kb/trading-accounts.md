---
id: trading-accounts
title: Trading accounts — trades of public accounts as analysis data, and entry timing as case leads
kind: feature
pages: /ontology, /cases
nodes:
err_codes: account-trades-csv-invalid, account-trades-too-large, ontology-link-not-allowed
tools:
tags: trading account, compte de trading, Myfxbook, trades, transactions, grid, grille, copy, entry, entrée, release, publication, gold, or, XAUUSD, pseudonym, pseudonyme
---
The **Trading accounts** pack (Knowledge › Ontology › Packs) holds the trades of public trading accounts, as their authors published them on a tracking platform, next to the price bars and the release calendar. PNEX only reads: it never copies a position, never places an order and never logs into someone else's account. It is an aid to analysis, not investment advice.

## Importing trades
Trades arrive as a CSV file sent by a client of the organization (for example an analysis script with an API token), one file per account, the account named as `myfxbook:<account id>`. The file is the canonical form: one trade per row, `symbol`, `side` (buy or sell), `lots`, `open_at` and `open_price` required, and `ticket`, `close_at`, `close_price`, `sl`, `tp`, `profit`, `pips`, `commission`, `swap`, `magic`, `comment` when known; instants carry their time zone offset. A file with a faulty cell is refused whole, with the line and the column (`account-trades-csv-invalid`); a file too large is refused with its limits (`account-trades-too-large`).

The import creates the *Trading account* object when it does not exist yet, and links it to each *Instrument* it traded (**trades**, with the number of trades and the period). Sending the same export again adds nothing: a trade already stored keeps the date PNEX first learnt it.

## Read as known at a date
A trade counts as known from its import, never from its opening, unless the platform proves live publication: a backtest at an earlier date does not see it. Such trades are flagged **backfilled**.

## Entry timing in a case
In a case holding an instrument, **Detect** proposes the accounts whose entries on that instrument fall before a move of at least the threshold, or around a release or a central bank decision of the calendar, when enough of them do (count and share, in the pack's **Settings**; prices from the source named in **Price source of the instruments**). The proposal gives figures only: entries, those before a move and how many went the move's way, those around a release, and their profit. Retaining it adds an event to the case with the account and the instrument. No intention is attributed.

## Never a person
The display name of an account is a pseudonym on the platform. A trading account can never be linked to a *Person*, whatever the link type: the link is refused (`ontology-link-not-allowed`).

## Deleting
Erasing a trading account (object page › Erase) deletes all its stored trades.
