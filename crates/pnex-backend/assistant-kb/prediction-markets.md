---
id: prediction-markets
title: Prediction markets — event probabilities as model inputs, and early bets as case leads
kind: feature
pages: /ontology, /market-decisions, /cases
nodes:
err_codes: as-of-required, as-of-unsupported
tools:
tags: prediction market, marché de prédiction, Polymarket, probability, probabilité, odds, cote, resolution, résolution, dispute, litige, oracle, bears on, porte sur, wallet, early bet, mise précoce, Brier, calibration, proxy, Fed, ceasefire, cessez-le-feu, sanctions
---
The **Prediction markets** pack (Knowledge › Ontology › Packs) reads questions on real events priced by a public prediction market: the implied probability that the Fed cuts rates, that a ceasefire holds, that sanctions are adopted. PNEX only reads public data: it never bets, never places an order and never holds a wallet key. It is an aid to analysis, not investment advice.

## Following markets
In the pack's **Settings**, list the markets to follow in **Markets followed** (their slugs or condition ids, separated by commas). Each one becomes a *Prediction market* object with its question, its full resolution rules, its outcomes and its end date; its probabilities are read every hour, every minute for the markets also listed in **Markets read every minute**. **Days of history at the first reading** sets how far back the first reading goes.

The platform's sites are blocked in France. To reach them, an owner or admin stores the organization's own proxy in **Secrets** under the name `polymarket_proxy` (an `http://` or `https://` proxy address, credentials included if needed). Without that secret the server calls the platform directly; with a secret it cannot read, nothing is read (never a silent direct call).

## Resolution
A market is resolved only by a **final** state. Each state the platform shows is kept as a dated *Market resolution* object: **proposed**, **disputed** (as many times as it happens), **final** with its outcome, or **indeterminate**. A reading at an earlier date sees only the states known then: a market in dispute at that date is unresolved, never resolved ahead of time.

## An input of the model
On the market's object page, a member opens a **bears on** link to an instrument, a country or a company, with the direction expected if the event happens (up, down, unknown). An external model connected with its access key (see *Model decisions*) reads, for an instrument and a date, the markets linked to it that were known and unresolved then, with their probability and its change over 1 hour and 24 hours and the days left; a point-in-time key can read them. A link opened today does not exist in a test of an earlier date.

## Against the market
A model may add to its decisions its own probabilities on markets. Once a market is resolved final, the page **Model decisions** shows, under **Against the prediction market**, the model's Brier score next to the market's at the same instant, with both calibration curves: a model that does not beat the market adds nothing.

## Early bets in a case
Add a prediction market to a case and run the detections: a wallet seen for less than 7 days, with at least 80 % of its volume on that market, that bought at least 10,000 USDC within 48 hours before a jump of 15 points or before the resolution, is **proposed** with those figures (the thresholds are in the pack's settings). It is a lead, never an accusation: no intention is attributed and no name is given. Retaining it creates a *Wallet* object linked to the market; a wallet is never an object otherwise. The wallet's age is its first trade among the trades PNEX collected (followed markets only), not its creation.
