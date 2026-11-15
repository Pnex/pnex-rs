---
id: labor-market
title: Labour market (France)
kind: feature
pages: /pipelines
nodes: 
err_codes: pipeline-input-missing, named-query-not-found
tools: 
tags: labour market, marché du travail, tension, métier en tension, BMO, besoins en main-d'oeuvre, recrutement, hiring, job offers, offres d'emploi, France Travail, ROME, FAP, département, department
---
The **Labour market (France)** pack loads public data on the jobs a territory finds hard to fill, as tables of the organization (never objects), each filled by a pipeline of the pack on its own schedule:

- **lm_bmo**: the yearly recruitment needs survey of France Travail (BMO): hiring projects, the hard ones and the seasonal ones per occupation (FAP 2021) and department, with the share of hard projects (`hiring_difficulty_pct`). Read every day; only a new file is loaded.
- **lm_tension**: the tension of each ROME job followed in each region followed, 1 (low) to 5 (high), and the factors behind it (working conditions, non-lasting jobs…). Each job and region is asked once a year; a long list (all ~1,900 jobs in 13 regions) fills over a day of short runs.
- **lm_offers_seen**: the open job offers of France Travail for the jobs and departments followed, seen each day. The recruiter's contact details are never read.

## Set it up
- Install the pack, then in its settings: **Regions followed** (INSEE codes, for example `84,11`) for tension, **Departments followed** (for example `69,38`) for offers, and **ROME jobs followed** (for example `N4101,H2913`) for both. Empty: that pipeline reads nothing.
- Tension and offers need the organization's own application on francetravail.io, subscribed to the job offers and labour market APIs. Put its client id and secret in the organization's **Secrets** under `lm_ft_client_id` and `lm_ft_client_secret`. Without them the runs of those two pipelines stop with a missing secret.
- **Address of the BMO file** is set to the latest edition; France Travail renames the file each year, change it when a new edition comes out (each spring).

## Read it
Named queries of the pack, run like any analysis query:
- **bmo** (a department): every BMO indicator of the latest edition.
- **tension** (a region): the latest tension of each job followed, and its factors.
- **job_postings**: each offer once, with its first and last day seen; an offer not seen on the latest day is closed.
- **company_offers** (part of a company name): the offers open today whose company name contains it. Many offers do not name their company (temp agencies, anonymous offers): they never match.

## Good to know
- The tension of the Dares (with its detailed components per FAP job) cannot be read automatically: its site refuses automated access. The France Travail API gives the same tension per ROME job.
- A share of BMO figures is hidden by statistical secrecy: the sums of a department can be lower than the real total.
- An offer's reason (replacement, retirement, growth) is guessed from words of its text; it is only a hint.
