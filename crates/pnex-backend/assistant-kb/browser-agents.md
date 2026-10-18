---
id: browser-agents
title: Browser agents
kind: feature
pages: /edges/browser-agents
nodes: 
err_codes: ext-permission-missing, ext-selector-not-found, ext-value-not-numeric, ext-login-required, ext-challenge, ext-rate-limited, ext-tab-hidden, extension-install-paired-elsewhere, extension-pair-refused, extension-dist-not-found
tools: 
tags: browser, extension, chrome, firefox, agent, mission, collect, scraping, web page, value, selector, onglet, navigateur, agent navigateur
---
The Browser agents page (Devices menu) lists the browser extensions paired with the organization and the missions they run. A browser agent is a Chrome extension shipped with PNeX: it runs missions in its own tabs, one tab per site, at a human pace. Pair one browser agent per computer.

## What you can do

- **Download the extension** (header button): the zip already holds this server's address.
- Install it: unzip, open chrome://extensions, turn on Developer mode, click **Load unpacked** and pick the folder. In the extension options, sign in, pick the organization and pair the browser: it appears in the **Browser agents** tab, Online while its browser runs.
- **Rename** or **Delete** a browser agent from its row. Deleting revokes its credentials; pair the browser again to bring it back.
- **Missions** tab, **New mission**: a page address, a CSS selector of the element that shows the value, what to read (`text`, or `attr:name` for an attribute), a series name and an interval (60 seconds at least). Leave the browser agent on "Any browser agent" or pin one.
- **Pause**, **Resume**, **Edit** or **Delete** a mission from its row.
- The value appears under the series name on the browser agent in Visualisation (the agent is a device of the organization).

## Good to know

- One browser agent runs a mission at a time; two never share a target. A site runs one mission at a time, at least 20 seconds apart, and each refresh varies a little.
- State **Blocked — the site is not allowed yet**: open the extension options and click **Allow** next to the site.
- **The selector matches nothing**: open the page in a browser and check the selector with the inspector; the page may also need a sign-in.
- **The value is not a number**: pick an element that shows only the number.
- **The site asks to sign in** or **shows a verification**: sign in or solve it in the agent's browser, then resume the mission. The agent never bypasses a verification.
- **This browser is already paired with another organization**: delete its browser agent in that organization first.
- Viewers see the page read-only.
