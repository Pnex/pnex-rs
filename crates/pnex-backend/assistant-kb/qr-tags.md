---
id: qr-tags
title: QR tags — labels that open an equipment, a part or a location
kind: feature
pages: /qr-tags, /q/:token, /qr-sheet, /scan
nodes:
err_codes: qr-tag-not-found, qr-tag-not-blank
tools:
tags: QR code, code QR, label, étiquette, sticker, autocollant, scan, scanner, print, imprimer, blank sheet, planche vierge, equipment, appareil, revoke, révoquer, maintenance
---
A **QR tag** is a label stuck on anything the org keeps an object for: a machine, a pump, a spare part, a stock location. Scanning it with a phone camera opens that object in PNEX. The QR holds only a short address with a random code, never a name or a number of the customer, and it only works for members of the organization that made it.

## Two ways to label
- **From an object**: open its page and choose **QR code**. The code appears; **Print** opens a sheet of labels to print.
- **Blank sheets, linked on site**: on the **QR tags** page (menu **Operate**), choose how many labels and **Blank sheet**, then print them. On site, scan a blank label: PNEX asks which object it is stuck on; type its name and choose **Link here**. From then on the label opens that object.

Labels print 4 per row, 40 per A4 page. The material (polyester, resistant to cold, grease or damp) is chosen when buying the sheets.

## Scanning in PNEX
**Scanner** (menu **Operate**) reads a label with the rear camera, in the browser or the mobile app, and opens its page. Where the browser cannot read codes, the phone's own camera app opens the printed address just as well. Allow the camera when asked.

## Revoking
A lost, torn or misplaced label is revoked from the **QR tags** page: it no longer opens anything. Stick a new one.

## Troubleshooting
- *This QR tag is unknown in this organization* (`qr-tag-not-found`): the label belongs to another organization, or the organization selected in PNEX is not the right one. Switch organization and scan again.
- *This QR tag is already linked or revoked* (`qr-tag-not-blank`): only a blank label can be linked. Scan opens the object it is linked to; to move it, revoke it and stick a new one.
- **Scanner says the camera is refused**: allow the camera for PNEX in the browser or phone settings, then **Try again**.
- **The scan opens the login page**: sign in, the label opens its object afterwards.
- **Linking is refused**: your role cannot link labels; ask an editor of the organization.
