---
id: profile
title: Profile
kind: feature
pages: /profile
nodes: 
err_codes: 
tools: 
tags: profile, account, language, langue, timezone, date format, theme, password, mot de passe, logout, sign out, version, certificate
---
The Profile page holds your personal account settings: identity (read-only), preferences such as interface language, timezone and date format, password change, sign-out, and the app and server versions.

## What you can do
- **Profile information**: shows your **Username** and **Email**. These fields are managed by the authentication server (Rauthy) and cannot be edited here.
- **Preferences**: choose the **Language** (English or Français; applied immediately), the **Timezone**, the **Date format** and the **Theme** (Light, Dark, Auto), then click **Save Changes**.
- **Account** → **Change password**: redirects to the authentication server's password reset flow.
- **Account** → **Log out**: ends your session.
- **About**: shows the application version, the API contract and the server version — useful when reporting a problem.
- **Server root certificate**: download pnex-ca.crt (or scan its QR code from a phone) and install it so phones and computers trust this server's HTTPS connection; the SHA-256 fingerprint lets you check it.

## Good to know
- The theme is stored in your profile, but its application to the interface comes with a later dark-mode release.
- Preferences are personal: they do not change anything for other members of your organizations.
- Organization settings (members, roles, LLM providers) are not here: they live in Organizations.
