---
id: markets
title: Markets — filings, figures, prices, reactions to filings and fictitious positions
kind: feature
pages: /ontology
nodes:
err_codes: import-contact-required, import-key-required, import-key-host
tools: market_figures, market_rule_test
tags: markets, marchés, subsidiaries, filiales, Exhibit 21, group structure, structure de groupe, investment, investissement, trading, ticker, SEC, EDGAR, FRED, filing, dépôt, 10-K, 10-Q, 8-K, reported figures, chiffres publiés, macro, inflation, known at, connu au, point in time, Tiingo, closing price, cours de clôture, event study, étude d'événement, reaction, réaction, abnormal return, rendement anormal, ECB, BCE, BIS, BRI, central bank, banque centrale, policy rate, taux directeur, Eurostat, HICP, IPCH, house prices, prix des logements, PortWatch, chokepoint, Suez, Hormuz, Ormuz, calendar, calendrier, FOMC, release, publication, ESEF, GDELT, press, presse, thesis, thèse, catalyst, catalyseur, paper position, position fictive, Binance, crypto, minute bars, bougies minute, price history, historique de prix, Dukascopy, forex, ticks, bid, ask, MetaTrader, MT5, Expert Advisor, broker, courtier, HistData, gold, or, oil, pétrole, index, indice
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

The link starts on the date of the report that declares it. When the latest report read no longer lists a subsidiary, its link is closed on that report's date: it reads "declared from this date to that date".

## LEI of a SEC company
The SEC does not publish LEIs. For each company created from its SEC number, PNEX looks its name up in GLEIF. When exactly one company has the same legal name, a proposal waits in **Proposals**: either the LEI itself, or, when another organization of the org already has that LEI, the link **is the same company as** between the two. Nothing is written or merged before a member accepts. The company shows **LEI lookup (GLEIF)**: *Proposed*, *Several candidates* or *Not found*; in the last two cases, set the LEI by hand if you know it.

## Prices and reactions
Closing prices need a Tiingo API key of your own: store it in **Secrets** under the exact name `markets_tiingo_key`, then run **Benchmark prices (Tiingo)** and **Daily prices (Tiingo)**. The key goes only to Tiingo's address. Its free plan covers personal internal use; a commercial use needs its commercial plan. Prices are read in turns, 40 instruments per run.

An instrument gets a **Reaction** tab:
- **Reaction to a date**: return and abnormal return around that day against the benchmark (setting **Benchmark of the event studies**), and the volume against normal.
- **Reaction to a kind of filing**: for example results (8-K item 2.02), over every followed instrument, next to dates drawn at random. Always read the number of events. Instruments no longer followed are missing, which can flatter a result.

## Europe
**European series (ECB)** reads the ECB series of the setting. **Central bank series (BIS)** reads the BIS series of the setting **Central bank series (BIS)**: by default the policy rates of the G10 central banks (Federal Reserve, ECB, Bank of Japan, Bank of England, Bank of Canada, Swiss National Bank, Reserve Banks of Australia and New Zealand, Riksbank, Norges Bank), daily since 2000; add a country with `WS_CBPOL.D.<country code>`. **European statistics (Eurostat)** reads the Eurostat series of its setting: euro area inflation by component and house prices by default. **Shipping chokepoints (IMF PortWatch)** reads the daily transits of the straits of the setting **Shipping chokepoints** (Suez, Panama, Hormuz, Malacca…), one series per strait. The setting **Calendars of releases and meetings** (`bls, fomc, ecb`) puts the coming US releases (CPI, employment) and the Federal Reserve and ECB decision days on the org's calendar each day; a moved date follows. **European reports (ESEF)** reads the annual and half-year reports of the LEIs of the setting **European companies followed (LEI)**. European closing prices are not covered yet.

## Minute history (crypto)
List Binance spot symbols in the setting **Symbols of the minute history (Binance)** (for example `BTCUSDT, ETHUSDT`) and, if needed, **First month of the minute history**. Their 1-minute bars are loaded from Binance's public files into the table `binance_spot_klines_1m` of the **Pipelines** page: one month at a time, the current month day by day, a few dozen files every five minutes until the history is complete, then the new days. Nothing becomes an object. Prices are kept as published, never adjusted or filled; a missing minute stays missing. Binance's terms are not checked for redistribution: keep the table inside the org.
- **A month is missing from the table**: its file had a wrong checksum or could not be read; it is fetched again on the next passes. A day Binance has not published yet is asked again for a week.
- **The table stops growing**: the org's table storage is full. Remove symbols or old tables.
- **Every trade**: list symbols in **Symbols of the trade history (Binance)** to load each trade into the table `binance_spot_trades`, one day at a time from **First month of the trade history** (one month back when empty). A busy symbol weighs hundreds of MB a day: the storage fills quickly, ask the platform admin to raise it.

## Tick history (forex)
List forex pairs in the setting **Pairs of the tick history (Dukascopy)** (for example `EURUSD, USDJPY`) and, if needed, **First month of the tick history** (twelve months back when empty). Their bid and ask quotes are loaded from Dukascopy's public files into the table `dukascopy_ticks` of the **Pipelines** page, one day at a time, two weeks of days every five minutes. Only pairs between EUR, USD, GBP, JPY, CHF, AUD, CAD, NZD, SEK and NOK are read; a metal or an index is ignored, its price scale is not known. These are a broker's quotes, not exchange prices, and their terms are not checked for redistribution: keep the table inside the org.
- **A day is missing**: one of its hour files could not be read; the day is fetched again on the next passes. Saturdays stay empty, the market is closed.

## Free 1-minute history (HistData)
List HistData codes in **Pairs of the HistData history** (for example `EURUSD, XAUUSD, SPXUSD`): forex pairs, gold and silver, oil (`WTIUSD`, `BCOUSD`) and indices (`SPXUSD`, `NSXUSD`, `UKXGBP`…), most since 2000 to 2010. Their free 1-minute bars go to the table `histdata_ohlcv_1m`: a closed year per file, the current year month by month, one file every five minutes, so a pair since 2000 takes about three hours. Prices are bid prices, times in UTC (HistData publishes New York time without daylight saving, converted). HistData publishes no volume (0) and no license: keep the table inside the org.
- **A year stays empty**: it is before the pair's first year, or HistData has not published it yet (asked again for a year).

## MetaTrader 5 bridge
To bring a broker's prices from MetaTrader 5: on the **Pipelines** page, create a **webhook** connector (for example `mt5`) and copy its URL and secret, shown once. In MetaTrader, allow WebRequest for the PneX address (Tools › Options › Expert Advisors), attach the PneX bridge Expert Advisor to a chart and paste the URL and the secret in its inputs, with the symbols to send. Then put the connector's name in the setting **Webhook of the MetaTrader 5 bridge**. Closed 1-minute bars go to the table `mt5_ohlcv_1m` and ticks to `mt5_ticks`, every five minutes, times in UTC; the source column names the broker. The bridge only reads: no order is ever sent from PNEX.
- **Nothing arrives**: the terminal's journal says why (*HTTP 401*: wrong secret; *error 4014*: the address is not allowed for WebRequest). A call the bridge could not deliver is sent again.
- **A minute appears twice**: a call whose answer was lost is sent again; keep a **unique** check on instrument and time in the pipeline that reads the table.

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
