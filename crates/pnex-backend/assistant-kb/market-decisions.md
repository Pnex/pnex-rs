---
id: market-decisions
title: Model decisions — an external model's buy, neutral or sell, and how well it did
kind: feature
pages: /market-decisions, /ontology
nodes:
err_codes: market-decision-stale, market-decision-conflict, market-decisions-quota, market-bars-too-many-files, as-of-required, as-of-unsupported
tools:
tags: model decision, décision de modèle, model, modèle, runner, buy, sell, neutral, achat, vente, neutre, probability, probabilité, calibration, reliability, fiabilité, Brier, hit rate, réussite, triple barrier, triple barrière, ATR, bars, barres, H4, gold, or, XAUUSD, service account, compte de service, Python, causal chain, chaîne causale, exposed, expose, point-in-time, backtest
---
A model trained outside PNEX (for example in a Python project) can post its decisions to the organization: for one instrument and one moment, the probability of **buy**, **neutral** and **sell**. PNEX stores them, shows them and measures them. It is an aid to analysis, not investment advice: no order is ever placed from PNEX, and nothing links a decision to an MT5 terminal.

## Connecting a model
On the page **Access**, create a **service account** for the model and give it a role with `datasets:read` (to read the bars), `ontology:read` and `time-ranges:read` (series, calendar, events) and `market-decisions:read` and `market-decisions:write`. Writing decisions is the only write a service account can ever make. Then create an **access key** for it and paste it in the model's configuration; it is shown once.

The model reads the bars of an instrument as they were known at a moment: a bar counts only once it is closed, so the model never sees in real time what it could not see in training. Prices are those of one source (`histdata`, `mt5:<broker>`…), never mixed; a minute written twice counts once.

For training and backtests, give the model a **point-in-time key** (a tick box when creating the key): every read must then name its moment (`as_of`), so a forgotten date is refused instead of silently reading today's data, and the key can never write. Keep a normal key for the live runner.

The model can also read **candidate causal chains**: for each hourly price move of an instrument beyond its threshold (1.5 % by default; the instrument's own **Price move threshold** wins), the world events, supply disruptions (blockades, sanctions) and macro releases that came before it, through the links a member declared with **exposes** (a country, a company or a macro series → the instrument). Each chain has a confidence and a **lift** against a placebo (the same move pretended a day, two days and a week earlier). They are computed when read, as known at the moment asked, never stored and never a cause: an input for the model, which decides how much it weighs. Declare the links on the object pages of the ontology; tune the pairs (prior, maximal gap) in the **Markets** pack settings.

## What the server checks
- the three probabilities are between 0 and 1 and add up to 1;
- the data read is at most two intervals old (otherwise **the decision is too old**);
- the instrument has prices in the organization;
- one decision per model, version, instrument, interval and moment: the same one sent again is accepted once, a different one is refused;
- at most 2,000 decisions a day per organization.

The most likely option and the confidence (1 − normalized entropy) are computed by PNEX, never taken from the model.

## Reading them
- The page **Model decisions** (menu Knowledge) lists them, newest first, with their probabilities, and filters by model or instrument.
- Above the list, each model shows **Measured after the horizon**: how many decisions could be measured, the hit rate, the Brier score (0 is perfect) and a small reliability chart: dots on the dashed diagonal mean the stated probabilities come true as often as they say. A model drifting away from it is losing its calibration.
- On an instrument's **Prices** tab, the last decision of each day of each model is a dot: green for buy, red for sell, grey for neutral, fainter when less sure. A list above the chart keeps one model.

## How a decision is measured
When a decision declares barriers (take-profit and stop in ATR), PNEX applies the **triple barrier** to the bars of the same source after the moment of the decision: ATR = simple mean of the true range of the last `atr_period` closed bars; the upper barrier touched first gives buy, the lower one sell, none within the horizon neutral, both in the same bar neutral. A decision is measured only once all the bars of its horizon are closed. The model should label its training data with exactly this rule, or the measure compares two different things.
