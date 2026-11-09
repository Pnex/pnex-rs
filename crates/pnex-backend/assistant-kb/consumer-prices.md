---
id: consumer-prices
title: Consumer prices — shelf prices of followed products, shop by shop
kind: feature
pages: /ontology
nodes:
err_codes:
tools:
tags: consumer prices, prix à la consommation, shelf price, prix en rayon, product, produit, barcode, code-barres, EAN, shop, magasin, supermarket, supermarché, inflation, Open Prices, Open Food Facts, ODbL
---
The **Consumer prices** pack follows the shelf price of products, shop by shop, from Open Prices (Open Food Facts): prices recorded by contributors from receipts and price tags, each dated by the day it was seen.

## Following a product
In the pack settings, **Products followed (barcodes)** lists EAN barcodes separated by commas (for example `3017620422003`), and **Contact declared to the sources** gives a name and e-mail, as Open Food Facts asks of automated clients. The import **Product prices (Open Prices)** then runs every day: each product becomes a **Consumer product** object named after the product, and each shop one of its series (shop name, city, country and OpenStreetMap id), in the shop's currency. A discounted price is a series of its own, marked `[discount]`. The first read starts from the oldest price, a hundred at a time; the next ones go on from the last price known.

## Limits
Coverage depends on contributors: mostly France and Europe, uneven from one product to another. The data is under the Open Database License: cite Open Food Facts, and a database derived from it is shared under the same licence.
