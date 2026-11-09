---
id: trade-flows
title: Trade flows — raw vessel (AIS) and aircraft (ADS-B) positions in tables
kind: feature
pages: /ontology
nodes:
err_codes:
tools:
tags: AIS, vessel, navire, ship, bateau, position, MMSI, maritime, ADS-B, aircraft, avion, flight, vol, ICAO, adsb.lol, NOAA, Danish Maritime Authority, autorité maritime danoise, trade flows, flux physiques, dataset, jeu de données, table
---
The **Trade flows** pack loads raw positions into tables of the organization, a day per file, from historical files only: vessels from the Danish Maritime Authority and from NOAA (United States), aircraft from adsb.lol (worldwide).

## Starting a source
Each source has its setting: **Danish AIS from (day)**, **US AIS (NOAA) from (day)**, **Aircraft (adsb.lol) from (day)**, a first day as `YYYY-MM-DD`. Empty, the source is not read. From that day on, one file a pass is loaded until yesterday, then each new day as it is published. The tables are `ais_dk_positions`, `ais_noaa_positions` and `adsb_lol_positions`, listed with the other datasets.

## What to expect
- A Danish day is about 15 million vessel positions (base stations and aids to navigation are left out), about 300 MB stored; an adsb.lol day several GB. Everything counts in the organization's dataset quota: when it is full, the loading stops until an administrator raises it.
- NOAA publishes months late: its loading waits on the first day not yet published. A day the others have not published yet is asked again for a week.
- A file cut during its download or unreadable is never kept half: its day shows as failed and the rest goes on.

## Terms
NOAA's data is US public domain (cite the source). adsb.lol's is under the Open Database License (cite adsb.lol, share a derived database alike). The Danish Maritime Authority has its own terms, to read before any use outside the organization.
