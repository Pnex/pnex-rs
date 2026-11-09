---
id: mt5-terminals
title: MT5 terminals — a broker's prices from MetaTrader 5, read only
kind: feature
pages: /mt5-terminals, /mt5-terminals/:id
nodes:
err_codes: mt5-terminal-not-found, mt5-terminal-name-taken, mt5-terminal-version-conflict, mt5-terminals-limit, mt5-token-invalid, mt5-body-invalid, mt5-inbox-full, mt5-base-unknown
tools:
tags: MetaTrader, MT5, Expert Advisor, EA, MetaEditor, broker, courtier, terminal, terminaux, bars, bougies, ticks, bid, ask, gold, or, XAUUSD, forex, backfill, historique, live prices, cours en direct
---
To bring a broker's prices from MetaTrader 5: on the page **MT5 terminals** (menu Sources), register a terminal (a name, optionally the broker, server and account number, which are not secrets), then **Download the Expert Advisor**: the file carries this server's address and the terminal's own token, shown nowhere else; downloading it again revokes the previous one. In MetaTrader, allow WebRequest for the PneX address (Tools › Options › Expert Advisors), compile the file in MetaEditor and attach it to any one chart. The terminal then announces every symbol of its broker with how far back its history goes, and pushes the closed 1-minute bars of its selection, by default the whole catalogue, plus the ticks of the symbols chosen for ticks (none by default; `XAU*` for gold). The selection takes `*`, names or prefixes such as `XAU*`; the EA reads it within five minutes. **Ask for past bars** makes it send again up to 3650 days of 1-minute bars, as far as the broker keeps them. Bars go to the table `mt5_ohlcv_1m` and ticks to `mt5_ticks`, every five minutes, times in UTC; the source column names the broker. The terminal shows **Online** while its EA calls, **Late** after three minutes of silence, **Offline** after an hour. The EA only reads: no order is ever sent from PNEX.

## When it does not work
- **Cannot register another terminal**: an organization has 20 terminals at most; delete one it no longer uses.
- **The status stays Never connected**: WebRequest is not allowed for the PneX address in MetaTrader (Tools › Options › Expert Advisors), or the Expert Advisor is not attached to a chart; the Experts tab of MetaTrader shows the refused calls.
- **The status turns Offline after working**: MetaTrader is closed or the computer asleep, or the Expert Advisor was downloaded again in PneX, which stops the previous one: attach the new file.
- **The catalogue stays empty**: the terminal announces its symbols when its Expert Advisor starts; restart it from the chart.
- **No past bars**: the broker keeps a limited history; the column History from of the catalogue shows how far back each symbol goes.
