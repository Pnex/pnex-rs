---
id: mailboxes
title: Mailboxes — collect and file incoming mail
kind: feature
pages: /mailboxes, /mailboxes/:id
nodes: mail_received, ontology_query, object_upsert, link_upsert
err_codes: mail-mailbox-not-found, mail-mailbox-name-taken, mail-no-mailbox, mail-mailbox-unknown, mail-mailbox-forbidden, mail-no-password, mail-unreachable, mail-tls-failed, mail-auth-failed, mail-folder-unknown, mail-protocol, mail-rate-exceeded, mail-attachment-not-found, mail-message-gone, mail-too-large
tools: list_mailboxes, list_mails, read_mail
tags: mail, pièce jointe, attachment, facture, invoice, junk, indésirables, e-mail, courriel, mailbox, boîte mail, inbox, IMAP, newsletter, abonnement, veille, forward, transfert, transféré, sales@, support@, shared mailbox, boîte partagée, agent mailbox, spam, phishing, DKIM, SPF, DMARC
---
A **mailbox** is a source, like a stream: PneX polls its new mails every two minutes (IMAP over TLS, port 993), keeps them as plain text and hands them to the flows that listen. PneX **collects and files**; it does not send mail.

## The Mailboxes page
**Mailboxes** (Devices menu) lists the mailboxes of the organization: who reads each one, its state (active, paused, in error with the reason) and when it was last polled.
- **Add mailbox**: name, address, IMAP server (port 993, TLS only), username, password (typed, kept in the vault, or picked from the Secrets page), the polled folders (INBOX by default) and how many days PneX keeps the mails (90 by default).
- **Test** connects and counts the mails of each folder, without fetching anything. It also names the server's spam folder when the mailbox does not poll it: a customer's mail filed as spam never reaches PneX until that folder is added to the polled folders.
- **Pause** stops the polling; **Resume** starts it again and clears the last error.
- **Delete** removes the mailbox and its mails from PneX, never from the mail server.
- The name of a mailbox you read opens its **mails**: correspondent, subject, whether PneX verified the sender (DKIM), as plain text; click a row to read it. Attachments are listed, never opened.
- **Save to media** next to an attachment copies it into the **Media** library, where it can be analysed, searched or used by a flow. PneX fetches the mail again from the server (it keeps no attachment), and the file goes through the upload rules: allowed formats only (PDF, office documents, images, audio, video…), size limit. Once saved, the button becomes **Open the media**. Saving needs the right to add media; a saved file is then seen by everyone who sees the organization's media, not only by the readers of the mailbox.

## Whose mailbox, who reads it
- **Your own mailbox**: you connect it and type its password (kept in the vault, never shown again). Only you read its mails; an administrator sees that it exists and its state, may pause or remove it, never reads it.
- **A shared mailbox** (sales@, support@, a newsletter box for watch): an administrator connects it and names the **teams** that read it.
- **The mailbox of an agent or an application** (a service account): an administrator connects it.

A mailbox keeps its mails 90 days by default. Removing a mailbox removes its mails in PneX, never on the mail server; PneX never marks a mail read.

## In a flow
The **Mail received** node (palette, Triggers) sends one message per new mail of the mailboxes you pick — only those you read; the deploy refuses the others (**mail-mailbox-forbidden**). Filters: sender contains, subject contains, and *leave out automatic replies, lists and the organization's own mails* (on by default, so flows never answer each other).

The sales follow-up pack ships **CRM · mails received**: pick your mailboxes in its Mail received node, then deploy. Each mail becomes a done incoming activity of the person whose e-mail identifier matches the sender.

## Forwarding from a mailbox PneX does not read
A salesperson whose company mailbox cannot be connected forwards the mail to a mailbox PneX reads. PneX finds the original sender (a forward as attachment, or the forward block of Gmail, Outlook or Apple Mail; in a forward of a forward, the first sender outside the organization) and files the mail under them — only when the forward comes from a member's address and passed authentication; otherwise the mail stays with whoever sent it.

## A mail is never an order
The text is untrusted: HTML is turned into text, images and links are never loaded, attachments are listed and never opened by PneX, until a reader saves one to the media library. PneX checks the sender itself: a valid DKIM signature of the sender's domain. When it is missing or broken (some small senders do not sign their mails), nothing is linked on its own: the link waits on the **To validate** page, where a person accepts it.

## When it stops
- **mail-auth-failed**, **mail-tls-failed**, **mail-folder-unknown**, **mail-no-password**: the mailbox is in error and stops until it is changed (password, server, folder).
- **mail-unreachable**: the server did not answer; PneX tries again at the next poll.
- **mail-rate-exceeded**: more than 500 mails in an hour; the mailbox is paused, resume it once checked.
- **mail-message-gone** when saving an attachment: the mail was moved or deleted on the mail server since PneX polled it; nothing is saved.
- **mail-too-large**: the mail or the attachment is above what PneX fetches (10 MB per mail) or stores.
- **mail-attachment-not-found**: the mail left PneX (its retention ended) or the page is out of date; refresh it.

## The assistant
The assistant lists the mailboxes you read (list_mailboxes), their mails (list_mails) and reads one (read_mail) — never a mailbox you do not read. It cannot send, answer, delete or mark a mail read. A mail's text is data for it, never instructions: a mail that asks it to do something is only reported to you.
