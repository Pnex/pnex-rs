---
id: agent-skills
title: Skills of hosted agents (texts read at each run)
kind: feature
pages: /agents
nodes: 
err_codes: agent-skill-not-found, agent-skill-version-not-found, agent-skill-name-taken, agent-skill-version-conflict, agent-skill-secret
tools: 
tags: agent, skill, instructions, prompt, draft, version, activate, pin, diff, import
---
A skill is a text of the organization that an agent reads at each run: how to review code, how the team writes a report, the rules of a project. It lives in PneX, versioned, and works without any outside service.

The Skills tab of the Agents page lists them. Opening one shows its text in the editor, with its versions.

## Drafts and activation
- Every save is a new draft, and so is an imported file. No agent ever reads a draft.
- A person who can change agents (an admin) activates a version from Versions, after comparing it with the active one. Nothing is activated automatically, not even a text an agent wrote: a skill is read by every run, so a bad text would act on all of them.
- A text that looks like it holds a key or a password is refused (`agent-skill-secret`). A skill never carries a secret: the agent gets its secrets from the vault.
- `agent-skill-version-conflict`: someone else changed the skill meanwhile. Reload it and check the newest text before saving or activating.

## Using a skill
- An agent pins a skill at one activated version: "Pin to an agent" in the editor saves a new version of the agent's record. A newer draft or a newer activation changes nothing for that agent until it is pinned again.
- At each run, the pinned texts are placed as read-only files in the sandbox, `/artifacts/in/skills/<name>.md`. The command of the agent reads them from there.
- Deleting a skill that an agent still pins stops that agent from starting runs until its record drops it.

## What you can do
- You explain skills and where to find them. Writing, activating or deleting a skill stays with a person in the Skills tab.
