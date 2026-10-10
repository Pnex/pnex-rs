---
id: geo-providers
title: Geo providers (basemap, geocoding, routing)
kind: feature
pages: /orgs/current, /map
nodes: 
err_codes: geo-provider-forbidden, geo-provider-not-found, geo-provider-name-taken, geo-not-configured, geo-rate-limited, geo-provider-failed
tools: 
tags: photon, graphhopper, api key, clé, maptiler, google, mapbox, map, carte, basemap, fond de carte, tiles, tuiles, style, maplibre, geocoding, géocodage, address, adresse, nominatim, valhalla, route, itinéraire, routing, provider, fournisseur
---
Your organization plugs in its own map engines, like it brings its own LLM: the **basemap** under the map, a **geocoder** (address → position, position → address) and a **router** (itinerary). PneX supplies none: until a provider is added, the map shows a plain background with "No basemap: add a basemap provider…", and address search and routing are unavailable.

## What you can do
In the **Geo providers** section of the organization detail (Owner or Admin):
- **Add a provider**: **Name**, **Type** (Basemap; Nominatim or Photon for geocoding; Valhalla or GraphHopper for routing), the **URL** (a MapLibre style URL for a basemap, e.g. from MapTiler; the service root otherwise), then tick the **Uses** it serves and, for each, **Default** to make it the one the organization uses.
  - **API key (optional)**: typed (stored encrypted in the secrets vault) or picked from the Secrets page. It is added to the provider's URLs as the parameter named in **URL parameter name of the key** (`key` by default, as Google and MapTiler expect; Mapbox uses `access_token`).
  - A basemap may have a **Dark style URL**. Geocoders and routers accept **Parameters** (`key=value`, e.g. `accept-language=fr` for Nominatim, `lang=fr` for Photon, `language=fr` for the route instructions), a **Max requests per second** (1 by default, the public Nominatim policy) and whether the provider allows keeping results.
- **Test**: loads the basemap style, or geocodes / routes a known place; a toast shows the latency or the reason of the failure.
- **Edit** or **Delete** (**Confirm delete**) a provider.

The first provider of a use becomes its default; ★ marks the defaults in the table.

## On the map
- **Search an address…** (top left of the map) geocodes through the organization's default geocoder: click a result to recentre the map, or **+ POI** to create a point of interest prefilled with that address and its coordinates.
- In **Add POI** mode, clicking the map prefills the location field with the address of that point when a reverse geocoder is configured.
- The map opens on the organization's default basemap. With several basemaps, a selector at the top right switches between them; your choice is remembered on this device.

## Good to know
- *geo-not-configured*: no default provider for that use — add one or tick **Default** on an existing one.
- *geo-rate-limited*: the provider's request budget is used up; retry after the indicated delay or raise **Max requests per second** if your provider allows it.
- *geo-provider-failed*: the provider is unreachable or answered unexpectedly — use **Test** on the provider to see the reason.
- A basemap key is a **browser key**: the browser loads the tiles itself, so every member can see it. Restrict it to your PneX address in the provider's console (as for any Google Maps or MapTiler key). Geocoding and routing keys never leave the server.
- The server refuses provider addresses that point at itself or at its internal services; use the LAN address of the machine running the engine.
- Members and Viewers see the list and use the map, but cannot change providers. Provider names are unique within the organization.
