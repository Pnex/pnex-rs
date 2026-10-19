---
id: llm-providers
title: LLM providers (assistant, flows, file analyses)
kind: feature
pages: /orgs/current
nodes: 
err_codes: access-denied, llm-provider-not-found, llm-provider-name-taken
tools: 
tags: llm, ai, assistant, used for, usage, flows llm, separate llm, anthropic, claude, openai, api key, model, provider, ia, configure assistant, bring your own llm, byo
---
The assistant runs on an LLM that your organization provides: PNeX itself supplies none. LLM providers are configured in the organization detail (Organizations → Manage, or the active organization's detail), with the API key stored in the secrets vault.

## What you can do
In the **LLM providers** section of the organization detail (Owner or Admin):
- **Add a provider**: fill **Name**, **Type** (Anthropic or OpenAI-compatible), **Base URL** for OpenAI-compatible (the version root, e.g. https://api.openai.com/v1, no trailing slash), **Model**, and the **API key**, then **Save**.
  - The API key is a secret field: **Type a value** (stored encrypted as a dedicated secret) or **Pick an existing secret** from the Secrets page.
  - Tick **Default provider** for the one used by everything not routed elsewhere.
  - **Used for**: tick **Assistant**, **Flows and file analyses (text)** or **Images (captions)** to give that usage to this provider instead of the default. This keeps, for example, a local model for flows and Claude for the assistant. Ticking a usage takes it away from the provider that had it.
- **Test**: sends a check to the provider; a toast shows "Provider working (N ms)" or "Test failed" with the reason (e.g. no API key set).
- **Edit** or **Delete** (**Confirm deletion**) a provider.

The table shows **Name**, **Type**, **Model**, **API key** (set, with the secret name, or missing), a **Default** badge and one badge per usage given to the provider.

## Good to know
- Each usage runs on the provider it is given to, else on the default provider: the assistant, the flow nodes `llm_text` and `media_fuse` and the summaries of files (text), `vision_caption` (images). Deleting a provider sends its usages back to the default.
- Without any provider the assistant shows "Assistant not configured for this organization: add an LLM provider in Organizations → org detail."
- There is no platform-wide fallback: every organization configures its own.
- Members and Viewers see the list but cannot change it.
- The server refuses provider addresses that point at itself or at its internal services (loopback, link-local, cloud metadata, single-word host names such as `ollama`): use the LAN address of the machine running the model (for example `http://192.168.1.20:11434`). The platform administrator can allow a specific host name.
- The key is never shown again; replace it from the provider form or from the Secrets page. Its usage appears in the Secrets page **Used by** column as "LLM provider".
- Provider names are unique within the organization.
