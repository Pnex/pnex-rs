---
id: howto-weather-card
title: Show the weather on a dashboard
kind: howto
pages: /dashboards, /flows
nodes: weather, memory_write, metric
err_codes: 
tools: 
tags: weather, météo, forecast, prévisions, temperature, temperature outside, extérieur, rain, pluie, wind, vent, met norway, open-meteo
---
The weather is not fetched by the dashboard itself: a flow with a **Weather** node fetches it for a location and stores it in the org shared memory, and the dashboard's **Weather** card reads those memory keys. The same data is then reusable in other flows and dashboards.

## Steps
1. In **Automation › Flows**, create a flow and add a **Weather** node (no input needed: it triggers itself). Set **Provider**, **Latitude**, **Longitude** and **Refresh (minutes)** (10 to 1440, 30 by default).
2. Wire its **current** output to a **Memory write** node with a **Key** such as `weather.current`, and its **7 days** output to another **Memory write** with `weather.daily`. Give each a **Lifetime** longer than the refresh interval.
3. Optionally wire an output to a **Metric** node to keep a history (one series per numeric field) for charts.
4. **Deploy** the flow. The node status shows "Up to date" after the first fetch.
5. In **Dashboards**, add the **Weather** card (palette group **Sensors**), and in the inspector pick the **Current conditions key** and the **7-day forecast key**. **Save**.

## Good to know
- **MET Norway** is free and allows commercial use; **Open-Meteo** is for non-commercial use only.
- If the memory lifetime is shorter than the refresh interval, the card empties between two fetches.
- "Weather unavailable" on the node: the provider could not be reached; it retries automatically.
- Single values (outside temperature, wind) can also be shown with a Value or Gauge widget using the **Memory (flows)** source and a field.
