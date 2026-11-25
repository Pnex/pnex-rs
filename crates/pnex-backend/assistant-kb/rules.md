---
id: rules
title: Rules
kind: feature
pages: /rules
nodes:
err_codes: rule-not-found, rule-name-taken, rule-version-conflict
tags: rule, règle, propose, proposer, work order, ordre de travail, alert, alerte, threshold, seuil, replay, rejeu, notification, watch, surveillance
---
A rule watches a condition and notifies when it turns true. On **objects**: the objects of a type that match filters (an opportunity over an amount, a status set to late); each object that starts matching is announced once. On a **series**: a measure of a device that stays above or below a threshold for some minutes.

Rules are evaluated every five minutes with the rights of the member who saved them last: a rule never sees what its author may not read, and rests if its author leaves or loses the right. The notification goes in-app, or through a notification channel of the organization. A rule never writes anything itself.

A rule may also **propose an object** each time it fires: an object of a chosen type (a work order, for example), with a title and fixed values, linked to the object that fired or to the device of the measure. It waits in **To validate**, where a member accepts or rejects it. In the title, `{value}` becomes the value of the measure. Proposing needs the right to file proposals.

A rule on **Market event** objects may send the **daily session note** instead of a notice per event: tick the box under the channel. The organization's AI writes one note per market session from the moves the rule matches, in the app and on the rule's channel.

**Replay** shows when a rule would have fired over the last hours or days, from the history of the objects and the measures, without sending anything: a way to tune the threshold before turning a rule on.

## Troubleshooting
- **A rule never fires.** Its author left the organization or lost the right to write rules: save it again. Replay it on a longer window to check the condition.
- **The replay says the history is incomplete.** The history store did not answer: try again later.
