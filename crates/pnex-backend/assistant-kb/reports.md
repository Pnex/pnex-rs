---
id: reports
title: Reports written by a flow
kind: howto
pages: /flows
nodes: llm_text, page_write, page_pdf, inject, pnex_notify
err_codes: ai-not-configured
tools: 
tags: report, rapport, weekly report, rapport hebdomadaire, summary, synthèse, résumé, llm, ai text, texte ia, cron, schedule, planification
---
A report is written by a flow: a trigger on a schedule gathers data, the organization's AI writes the text, and the text becomes a page (Pages) that can be exported as PDF and sent. There is no separate report studio.

## Build it
1. **Inject** with a **cron** schedule (for example every Monday at 7:00) starts the flow.
2. Gather the data of the period: an **HTTP fetch**, a **Memory read**, the descriptions of a **Media fusion**, events…
3. **AI text**: write the **Instruction** ("Write the weekly report of these events: what happened, when, anything unusual"), and if needed the **Data** path inside the message. The answer is Markdown (`payload.text`), marked as AI-generated.
4. **Write a page**: a **new page for each message** (title such as "Weekly report {week}"; {date}, {time}, {week} are replaced) or **append at the end of a page** (a running log). Each write freezes a named version ("{date}" by default) marked "written by a flow" and, when the text comes from the AI, "Holds AI text · review it before it is sent".
5. **Page PDF** after **Write a page**: output **PDF ready** gives the file of the media library; output **To approve** fires when the version holds AI text not yet approved: wire a **Notification** there ("weekly report to review", with the page link).
6. A person reads the version in **Pages** and clicks **Approve** (the **PDF** button of a version downloads it at any time).
7. To send it once approved: a second flow, **Inject** every 15 minutes → **Page PDF** with **The latest sendable version of a page, once** → **Notification** with the PDF attached. A version is never sent twice by the same node.

## Attach the PDF to the e-mail
- Use a notification template with a variable, for example `report` (subject "Report {{ report.title }} v{{ report.version }}"), and wire the **PDF ready** output of **Page PDF** to the `report` row of the **Notification**.
- In the **Notification**, set **PDF attachment** to `report` (the variable holding the Page PDF output) or `asset_id` when the template has no variable.
- The **trigger** row is mandatory: wire an **Inject** that sends `true` once when the flow starts, so every new PDF is sent.
- Only e-mail (SMTP) channels carry the file; other channels get the message alone. If the file cannot be read, the e-mail is not sent (no report without its PDF) and the delivery shows as failed in the notification journal.
- Only PDFs made by a **Page PDF** node can be attached, never another file of the media library.

Unticking **Approval required for AI text** sends AI reports without review; the PDF then says "not reviewed". Keep it ticked unless the data cannot carry hostile text.

## Good to know
- The organization needs an AI provider (Organization settings); without one the node shows "AI call failed" with the code `ai-not-configured`.
- Every call counts in the organization's daily AI quota, shared with image captions and media fusion (2000 per day).
- The data is passed as data: instructions written inside it (a transcript saying "ignore your rules…") are ignored. Still, an AI report is meant to be read by a person before it leaves the organization.
