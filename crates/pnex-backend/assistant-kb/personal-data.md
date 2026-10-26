---
id: personal-data
title: Personal data — masked fields, objection, erasure and the retention queue
kind: feature
pages: /proposals
nodes:
err_codes: prospection-opposed, access-denied
tools:
tags: personal data, données personnelles, GDPR, RGPD, objection, opposition, erasure, effacement, right to be forgotten, droit à l'effacement, retention, conservation, durée de conservation, legal basis, base légale, masked, masqué, hidden, caviardé, sensitive, sensible, role, rôle
---
Some properties are classified **personal** or **sensitive** by the pack that ships them: a person's name, aliases, role and notes, the value of an identifier (e-mail, phone, account), the position held at a company, the persona or the role in an opportunity.

## Masked fields
A role without the permission **Personal data · View** sees a person's name replaced by `••••` and the first characters of its id, and the personal properties and link attributes left out; the object lists what was hidden. **Sensitive data · View** is given to administrators only. Give or take these permissions on the **Access** page with a custom role (for instance a deny on personal data for a partner). The graph and the hierarchy still show names for now.

## Objection
On a person's page, **Record their objection to prospecting** puts the person on the organization's opposition list, with its name at each account it is linked to and its profile addresses: profile captures are refused, the follow-up plans skip it, and a new link that would recreate the same name at the same account is refused (**prospection-opposed**). *Objected on* shows the date. There is no undo: the objection is the person's right.

## Erasure
Erasing a person removes its personal properties, link attributes and name, keeps an empty shell (`∅` and the first characters of its id) so the links of other objects hold, deletes its captured profile texts and the mails it wrote, and erases the identifiers it uses. It also records the objection. The shell can no longer be edited nor linked (error ontology-erased). Its history is then shown without what was erased. Erasure is irreversible and needs the delete permission.

## Retention queue
With **B2B account mapping** installed, a person with no change, no link and no profile capture for the retention period (setting *Retention (months)*, 36 by default, from the pack's settings) is proposed for erasure on the **To validate** page, with its last activity. Accept to erase, reject to keep: the refusal restarts the period. An organization rule never accepts an erasure: a person decides. The setting *Legal basis* of the same pack is the basis shown for the contacts' data.
