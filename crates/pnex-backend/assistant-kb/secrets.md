---
id: secrets
title: Secrets
kind: feature
pages: /secrets
nodes: http_fetch, pnex_notify
err_codes: secret-not-found, secret-in-use, secret-not-referenced, secret-name-taken, secret-write-forbidden
tools: 
tags: secret, vault, password, token, api key, credential, coffre, mot de passe, clé, wifi password, encryption
---
The Secrets page is the organization's vault: tokens, passwords and API keys, encrypted at rest. It lists every secret with what uses it, but a value is never shown again once saved — it can only be replaced.

## What you can do
- **New secret** (Owner or Admin): give a **Name** (e.g. telegram-oncall), an optional **Description** and the **Value**.
- **Edit** (Owner or Admin): rename, change the description, or type a **New value** (leave it empty to keep the current one).
- **Delete** (Owner or Admin): click **Delete** then **Confirm?**. Refused while the secret is still used ("This secret is still in use…"): remove it from its consumers first.
- **Search a secret…** filters the list.
- The **Used by** column links to each consumer: a notification channel, a flow (e.g. an HTTP node), a WiFi, an LLM provider. "Unused" means nothing references it.

## Secret fields elsewhere
Forms that need a secret (notification channel, HTTP node authentication and proxy, WiFi, LLM provider) show a secret field with two modes:
- **Type a value**: creates or replaces a dedicated secret named after its place (Owner or Admin only).
- **Pick an existing secret**: choose any secret of the organization. Members can do this without ever seeing the value.

## Good to know
- Roles: Owners and Admins create, change and delete; Members only pick existing secrets in forms; Viewers see the names only. Nobody can read a value back.
- A changed value is used by flows at their **next deploy**, not live.
- Changing a WiFi password does not rebuild firmwares: devices already flashed keep the old credentials and must be reflashed one by one.
- Function nodes (JavaScript/Starlark) have no access to secrets; a flow stores a reference to the secret, never the value itself.
