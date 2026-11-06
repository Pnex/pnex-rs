---
id: markets
title: Markets — filings, figures, prices, reactions to filings and fictitious positions
kind: feature
pages: /ontology
nodes:
err_codes: import-contact-required, import-key-required, import-key-host
tools: market_figures, market_rule_test
tags: markets, marchés, subsidiaries, filiales, Exhibit 21, group structure, structure de groupe, investment, investissement, trading, ticker, SEC, EDGAR, FRED, filing, dépôt, 10-K, 10-Q, 8-K, reported figures, chiffres publiés, macro, inflation, known at, connu au, point in time, Tiingo, closing price, cours de clôture, event study, étude d'événement, reaction, réaction, abnormal return, rendement anormal, ECB, BCE, ESEF, GDELT, press, presse, thesis, thèse, catalyst, catalyseur, paper position, position fictive
---
The **Markets** pack reads primary sources instead of press articles: what a company files with the SEC and what public statistics publish. It is an aid to analysis, not investment advice: never present a figure as a recommendation.

## Setting it up
Install the pack on the **Packs** tab of the ontology, then fill its settings:
- **Contact declared to the sources**: a name and an email, sent with each request. Required.
- **US tickers followed**: the companies to read, separated by commas. Nothing else is created.
- **Macroeconomic series (FRED)**: the series codes.

Then, on the **Imports** tab, run in this order: **Companies followed (SEC)**, **Filings (SEC EDGAR)**, **Reported figures (SEC EDGAR)**, **Macroeconomic series (FRED)**. They then run by themselves.

## What you get
- A company gets its **filings** (10-K, 10-Q, 8-K, 20-F, 40-F, 6-K). Their text is downloaded in the background, a few documents at a time, and becomes a searchable document linked to the filing. Insider and ownership forms are not read.
- A company or a series gets a **Figures** tab: one indicator at a time, with the day each value became known.

## Subsidiaries (Exhibit 21)
For each annual report (10-K) of a followed company, PNEX also reads its **Exhibit 21**, the list of subsidiaries the group declares, worldwide. Each subsidiary becomes an organization with its country and its jurisdiction (**Incorporated in**), linked to the group by **is a subsidiary of** with the nature *Declared* and the exhibit's address as source. A subsidiary already known by the same country and name is reused, never duplicated. The filing then shows **Subsidiaries (Exhibit 21)**: *Read*, *No Exhibit 21*, or *Unreadable* when the list has a form PNEX does not understand. A jurisdiction PNEX does not know leaves its row out.

## Prices and reactions
Closing prices need a Tiingo API key of your own: store it in **Secrets** under the exact name `markets_tiingo_key`, then run **Benchmark prices (Tiingo)** and **Daily prices (Tiingo)**. The key goes only to Tiingo's address. Its free plan covers personal internal use; a commercial use needs its commercial plan. Prices are read in turns, 40 instruments per run.

An instrument gets a **Reaction** tab:
- **Reaction to a date**: return and abnormal return around that day against the benchmark (setting **Benchmark of the event studies**), and the volume against normal.
- **Reaction to a kind of filing**: for example results (8-K item 2.02), over every followed instrument, next to dates drawn at random. Always read the number of events. Instruments no longer followed are missing, which can flatter a result.

## Europe
**European series (ECB)** reads the ECB series of the setting. **European reports (ESEF)** reads the annual and half-year reports of the LEIs of the setting **European companies followed (LEI)**. European closing prices are not covered yet.

## Press
**Press leads (GDELT)** lists the press naming a followed company. An article is a lead, marked *Unconfirmed*: it feeds no figure. Check it against a filing. If a company is missed, add the name the press uses in **Names in the press** on the company (for example Nvidia).

## Investment thesis
The **Investment thesis** pack adds to a case: direction, what would prove it wrong, horizon, planned size, closing review. Link the case to its instrument (**is about**), add **Catalysts** with their due date and **Fictitious positions**. A position has a **Result** tab measured on closing prices. No order can be placed from PNEX. Take a snapshot of the case at each decision to freeze the thesis.

## Assistant
The assistant reads these figures (`market_figures`) and tests a kind of filing (`market_rule_test`). It cites the source and gives no recommendation.

## Market moves and world events
Abnormal sessions of index, oil, rate or VIX series, world events and their **Session** tab are described in the sheet *Market moves and world events*.

## Reading the past
**Known on** shows the values as they were public at the end of that day. A later correction adds a value; it never replaces the earlier one. A figure counts as known at the end of its filing day. A macroeconomic value read for the first time has an unknown publication date: it shows *publication date unknown* and stays out when you choose a date.

## Troubleshooting
- *This source requires a declared contact* (`import-contact-required`): the contact setting of the pack is empty. Fill it, then run the import again.
- *This source takes an API key* (`import-key-required`): store the key in **Secrets** under the name `markets_tiingo_key`.
- *This import uses an API key and can only point at the address of its source* (`import-key-host`): the import's address was changed. Put back the address of the source.
- **Revenue stops years ago**: companies change the name of the indicator over time. Look at the other revenue indicators in the list, such as *Revenues*.
- **Two values for one period**: the source itself changed. It can be a real restatement, a rounding, or a tagging error in the filing: open the filing to check before relying on it.
