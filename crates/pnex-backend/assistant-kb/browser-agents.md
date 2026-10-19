---
id: browser-agents
title: Browser agents
kind: feature
pages: /edges/browser-agents
nodes: 
err_codes: ext-permission-missing, ext-selector-not-found, ext-value-not-numeric, ext-login-required, ext-challenge, ext-rate-limited, ext-tab-hidden, ext-drm-protected, ext-registry-full, extension-install-paired-elsewhere, extension-pair-refused, extension-dist-not-found, extension-video-not-found, extension-limit-reached, media-transcripts-unavailable, crawl-unreachable, crawl-robots-disallowed, crawl-blocked, ext-no-agent
tools: 
tags: browser, extension, chrome, firefox, agent, mission, collect, web page, value, selector, follow, account, x, twitter, youtube, tiktok, shorts, live, video, captions, subtitles, history, registry, onglet, navigateur, agent navigateur, compte, sous-titres, registre
---
The Browser agents page (Devices menu) lists the browser extensions paired with the organization, their missions and the video registry. A browser agent is a Chrome or Firefox extension shipped with PNeX: it runs missions in its own tabs, one tab per site or platform, at a human pace. Pair one browser agent per computer, ideally in a browser profile kept for it and signed in to the platforms it follows.

## What you can do

- **Extension for Chrome** / **Extension for Firefox** (header buttons): the zip already holds this server's address.
- Install it: Chrome, unzip, open chrome://extensions, turn on Developer mode, click **Load unpacked**; Firefox, open about:debugging, This Firefox, **Load Temporary Add-on**, pick manifest.json. In the extension options, sign in, pick the organization and pair the browser: it appears in the **Browser agents** tab, Online while its browser runs.
- **Rename** or **Delete** a browser agent. Deleting revokes its credentials.
- **Missions** tab, **New mission**, four kinds:
  - **Read a value**: a page, a CSS selector, a series name, an interval; the value appears on the browser agent in Visualisation.
  - **Keep the page**: the page's readable text becomes a document of the media library, a new version only when the text changes; it is searchable.
  - **Follow an account**: pick X, YouTube or TikTok and type the account. The agent collects the account's whole history (a little deeper at each visit, until the start of the account), then its new posts. By default it keeps posts, text and the platform's own captions (no file). Tick **Force the video collection** to download or record every video, then transcribed by PNeX.
  - **Record a live**: the page of a live; the agent records it in parts of 30 minutes while it is on air, each part a video of the media library.
- **Video registry** tab: every post the agents saw, once per organization (a post seen again is never downloaded or processed twice), with its state: text post, video not downloaded, transcript in the media library, video in the media library. **Open the review** opens the media with its transcript.

## Crawling a site and dated proofs
- Mission **Crawl a site**: the server (or a browser agent, for pages built by JavaScript) walks one declared site: robots.txt obeyed, sitemaps first, then the site's links, never off the site, one page every 2 seconds at most. Rules (words, phrases, `re:` expressions) mark the **relevant** pages; the others are only listed.
- **Pages** on the mission lists the registry: listed, relevant, proven, gone (missing from two crawls in a row, often the proof of a fix). The text of each relevant page is kept as a document whose versions can be compared.
- A relevant page new or changed gets a **proof** taken by a browser agent of the organization: full-page screenshot, the page's HTML without its scripts (download only) and its text, sealed with the server's time and fingerprints, chained to the previous proof. *Check the proof chain* recomputes it: a proof changed afterwards breaks it. The agent needs the right to capture the site (*ext-permission-missing*); without an agent online the proof waits (*ext-no-agent*).
- *crawl-robots-disallowed*: the site forbids robots; *crawl-blocked*: it refused the crawler (the crawl stops and never insists); *crawl-unreachable*: the site cannot be reached.

## Good to know

- Sign in to X, YouTube or TikTok in the agent's browser before following an account there.
- State **Blocked — the site is not allowed yet**: open the extension options and click **Allow** next to the site (X needs x.com and video.twimg.com; TikTok needs tiktok.com and its subdomains).
- **The site asks to sign in** or **shows a verification**: sign in or solve it in the agent's browser, then resume the mission. The agent never bypasses a verification.
- TikTok pauses its videos in a hidden tab: keep the agent's window in front, screen unlocked, while it reads TikTok.
- A video without a downloadable file (YouTube) is recorded in real time: it takes as long as the video. A long video is recorded in parts of 30 minutes (8 hours at most); the first part is the post's media, each next part a media of its own. A protected video (DRM) is never recorded.
- Platform captions are kept as a transcript of origin "platform"; **Retranscribe** on the review screen adds a PNeX transcript.
- One browser agent runs a target at a time; a site runs one mission at a time, at least 20 seconds apart.
- An organization pairs at most 20 browser agents and holds at most 200 missions: past that, delete one first. The video registry keeps at most 500,000 posts per organization: when full, account missions are blocked with **The video registry is full**.
- Viewers see the page read-only.
