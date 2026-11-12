---
id: real-estate
title: Real estate — every property sale in France (DVF) in a table
kind: feature
pages: /ontology, /admin/status
nodes:
err_codes:
tools:
tags: real estate, immobilier, DVF, valeurs foncières, property sale, vente immobilière, house prices, prix des logements, price per square metre, prix au mètre carré, commune, France
---
The **Real estate** pack loads every property sale recorded in France (DVF, published by the tax administration and geolocated by Etalab) into the table `dvf` of the organization, a year per file.

## Starting it
Like the other bulk loads, it only runs for an organization whose table quota a platform administrator has set, on the platform status page, section **Organizations**, column **Tables (quota)**.

Set **DVF sales from (year)** to a first year (`YYYY`); DVF keeps the last five, closed years only are loaded, one a pass. A year is about 3.7 million lines and 75 MB stored. A sale of several lots has several lines with the same mutation id: the value is the sale's, not the lot's, so a price per square metre is computed on sales of a single property.

## Indices
House price indices (BIS, Eurostat, FHFA) are in the **Markets** pack, next to rates and inflation.

## Terms
Licence Ouverte 2.0. The data must never be used to re-identify a person, nor exposed to search engines. Alsace-Moselle and Mayotte are not covered.
