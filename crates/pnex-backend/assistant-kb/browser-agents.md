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
- A browser agent is **personal**: it runs only the missions of the member who paired it, since it browses in that member's signed-in session. A mission waits for its creator's browser, even offline, and says so with *ext-no-agent* when none can run it.
- An admin can tick **Common agent** on their own browser, in a browser profile without any personal account: it then runs the missions of members without their own browser, and the proofs of the organization. Without a common agent, proofs are taken by the server.
- Members see their own browser agents and the common ones. An admin sees all of them and can disconnect any, but never sends a mission to a colleague's personal browser.
- Changing what a colleague's mission does makes it yours: it moves to your browser. When a member leaves the organization, the browser agents they paired are disconnected; edit their missions to take them over.
- **Rename** or **Delete** a browser agent. Deleting revokes its credentials.
- **Missions** tab, **New mission**, four kinds:
  - **Read a value**: a page, a CSS selector, a series name, an interval; the value is stored as a series on the browser agent, usable in dashboards.
  - **Keep the page**: the page's readable text becomes a document of the media library, a new version only when the text changes; it is searchable.
  - **Follow an account**: pick X, YouTube or TikTok and type the account. The agent collects the account's whole history (a little deeper at each visit, until the start of the account), then its new posts. By default it keeps posts, text and the platform's own captions (no file). Tick **Force the video collection** to download or record every video, then transcribed by PNeX.
  - **Record a live**: the page of a live; the agent records it in parts of 30 minutes while it is on air, each part a video of the media library.
- **Video registry** tab: every post the agents saw, once per organization (a post seen again is never downloaded or processed twice), with its state: text post, video not downloaded, transcript in the media library, video in the media library. **Open the review** opens the media with its transcript.

## Crawling a site and dated proofs
- Mission **Crawl a site**: the server (or a browser agent, for pages built by JavaScript) walks one declared site: robots.txt obeyed, sitemaps first, then the site's links, never off the site, one page every 2 seconds at most. Rules (words, phrases, `re:` expressions) mark the **relevant** pages; the others are only listed.
- **Pages** on the mission lists the registry: listed, relevant, proven, gone (missing from two crawls in a row, often the proof of a fix). The text of each relevant page is kept as a document whose versions can be compared.
- A relevant page new or changed gets a **proof** taken by a common browser agent of the organization: full-page screenshot, the page's HTML without its scripts (download only) and its text, sealed with the server's time and fingerprints, chained to the previous proof. *Check the proof chain* recomputes it: a proof changed afterwards breaks it. The agent needs the right to capture the site (*ext-permission-missing*); without a common agent online the proof waits (*ext-no-agent*), then the server takes it.
- *crawl-robots-disallowed*: the site forbids robots; *crawl-blocked*: it refused the crawler (the crawl stops and never insists); *crawl-unreachable*: the site cannot be reached.

## Capturing a person's profile

- On a **person** page, **Capture a profile**: paste one profile address (LinkedIn, X, Facebook, Instagram, TikTok, YouTube). Your own browser agent (or a common one if you have none) opens it once, in the session signed in within the agent, at a human pace. The person's page then lists each capture and where it stands (waiting, blocked, captured, stopped), and warns when your browser agent is offline. The page text becomes a document of that person (kept 30 days, then deleted) and the address an account identifier of the person. The capture is a default navigation rule of PNeX played in the browser agent: with no browser agent paired in the organization, it is refused ("This rule plays in a browser agent…") until one is paired; a capture waits at most 30 minutes for the agent.
- Only a profile page is accepted: searches, relation lists and "people also viewed" pages are refused (error "not one person's profile page"). At most 30 captures per network per day for the organization.
- The capture is always asked by a person in the app: the assistant cannot start one.
- **Record their objection to prospecting** (in the same window, confirmed twice): no capture of that person or of their profile addresses anymore, even if the person is deleted later.
- The platform's terms may forbid automated access; the suspension risk falls on the account signed in within the agent.

## Finding a person's profile

- On a **person** linked to their company (works for), **Find on LinkedIn**: your browser agent types the person's name and company in LinkedIn's search bar, in your own signed-in session, then the name with the company's other names, its group, a city of its sites, and the name alone, until a result fits. It opens the first profiles found, at a human pace. PNeX scores each one against the company (current position, past position, group, city) and explains the score.
- Above the score set by the prospecting pack (80 by default) the profile is attached to the person (account identifier, document kept 30 days); between the two scores the candidates wait in **To validate**; a profile where the company is only a past position (a job change) is always proposed, never attached alone. The person's page lists each search, its candidates, their score and what became of them.
- The prospecting pack also searches by itself, every hour, the people of the accounts whose status is target or prospect, best target score first: 20 searches a day by default (setting, 0 turns it off, 30 at most), never someone who objected, never a person already having a LinkedIn account, a person not found again after 90 days.
- "This person works for no company yet": link the person to their company first. Without a browser agent paired, the search is refused; a search your agent could not play (sign-in asked, site not allowed) stops with that reason.

## Capturing a company's employees

- On an **organization** page, **Capture employees (LinkedIn)**: paste the company's People page (`linkedin.com/company/<name>/people/`). Your browser agent (or a common one) opens it once in its signed-in session, scrolls the first screens at a human pace and reads them; the text becomes a document of the company, kept 30 days. The organization's page lists each capture and where it stands. Like a profile capture, it is a default navigation rule played in the browser agent, refused when the organization has none.
- Any other page is refused (error "not a company's employee list on LinkedIn"). At most 10 captures per day for the organization.
- With the B2B account mapping pack, the flow **Prospecting · captured employee lists** (deploy it once, needs an AI provider) turns each person named into a list contact (name, position) and proposes them as a contact at that company on the **To validate** page. Someone who objected at that company is never created again.
- The capture is always asked by a person in the app: the assistant cannot start one. The platform's terms may forbid automated access; the risk falls on the account signed in within the agent.

## Good to know

- Sign in to X, YouTube or TikTok in the agent's browser before following an account there.
- LinkedIn, X, Facebook, Instagram, TikTok and YouTube are accepted once, when the extension is installed. State **Blocked — the site is not allowed yet** on another site: open the extension options and click **Allow** next to the site; the mission starts again within seconds (X needs x.com and video.twimg.com; TikTok needs tiktok.com and its subdomains).
- **The site asks to sign in** or **shows a verification**: sign in or solve it in the agent's browser, then resume the mission. The agent never bypasses a verification.
- TikTok pauses its videos in a hidden tab: keep the agent's window in front, screen unlocked, while it reads TikTok.
- YouTube on a shared agent reads the latest videos and shorts of the account only: a hidden tab loads no more of the list. For the older history, set the agent's mode to dedicated to YouTube: it then brings its tab to the front while it reads.
- Two dedicated platforms on one computer: use one browser profile per platform, each with its own paired agent, and keep both windows visible side by side.
- A video without a downloadable file (YouTube) is recorded in real time: it takes as long as the video. A long video is recorded in parts of 30 minutes (8 hours at most); the first part is the post's media, each next part a media of its own. A protected video (DRM) is never recorded.
- Platform captions are kept as a transcript of origin "platform"; **Retranscribe** on the review screen adds a PNeX transcript.
- One browser agent runs a target at a time; a site runs one mission at a time, at least 20 seconds apart.
- An organization pairs at most 20 browser agents and holds at most 200 missions: past that, delete one first. The video registry keeps at most 500,000 posts per organization: when full, account missions are blocked with **The video registry is full**.
- Viewers see the page read-only.
