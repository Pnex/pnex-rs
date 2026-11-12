---
id: trade-flows
title: Trade flows — raw vessel (AIS) and aircraft (ADS-B) positions in tables
kind: feature
pages: /ontology, /admin/status
nodes:
err_codes:
tools:
tags: table quota, quota de tables, AIS, vessel, navire, ship, bateau, position, MMSI, maritime, ADS-B, aircraft, avion, flight, vol, ICAO, adsb.lol, NOAA, Danish Maritime Authority, autorité maritime danoise, trade flows, flux physiques, table
---
The **Trade flows** pack loads raw positions into tables of the organization, a day per file, from historical files only: vessels from the Danish Maritime Authority and from NOAA (United States), aircraft from adsb.lol (worldwide).

## Starting a source
These loads are several GB a day: they only run for an organization whose table quota a platform administrator has set, on the platform status page, section **Organizations**, column **Tables (quota)**. Until then the settings are kept and nothing is loaded.

Each source has its setting: **Danish AIS from (day)**, **US AIS (NOAA) from (day)**, **Aircraft (adsb.lol) from (day)**, a first day as `YYYY-MM-DD`. Empty, the source is not read. From that day on, one file a pass is loaded until yesterday, then each new day as it is published. The tables are `ais_dk`, `ais_noaa` and `adsb_lol_positions`, listed with the other tables.

## What to expect
- A Danish day is about 15 million vessel positions (base stations and aids to navigation are left out), about 300 MB stored; an adsb.lol day several GB. Everything counts in the organization's table quota: when it is full, the loading stops until a platform administrator raises it.
- NOAA publishes months late: its loading waits on the first day not yet published. A day the others have not published yet is asked again for a week.
- A file cut during its download or unreadable is never kept half: its day shows as failed and the rest goes on.

## Terms
NOAA's data is US public domain (cite the source). adsb.lol's is under the Open Database License (cite adsb.lol, share a derived database alike). The Danish Maritime Authority has its own terms, to read before any use outside the organization.
