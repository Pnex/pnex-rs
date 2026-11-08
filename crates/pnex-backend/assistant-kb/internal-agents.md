---
id: internal-agents
title: Internal agents (agents that run inside PneX)
kind: feature
pages: /agents, /proposals
nodes: agent_run
err_codes: agent-loop-detected, proposal-tool-failed, agent-run-llm-budget, agent-gateway-quota, agent-gateway-not-configured, agent-artifact-storage-unavailable, agent-run-output-missing
tools: 
tags: agent, internal, hosted, level, tools, proposal, validate, report, steps, loop, validator, rearm, who decides, autonomous, interne, hébergé, niveau, outils, étapes, rapport, validateur, réarmer, qui décide
---
An agent has a **Level**. A **Hosted** agent runs in a sandbox from the active build of an environment. An **Internal** agent runs inside PneX: no environment, no image, no command, no variable, no repository, no secret. It works with a closed list of the assistant's own tools, picked on its record, and the rights of its own identity (its service account and role), never those of the person who starts it.

## What you can do

- **Agents** page, **New agent**: pick **Internal** in **Level**, give a name, a timeout, a budget of model calls and the names of its inputs. Then open the agent: under **Tools**, tick what it may call; **Most steps** bounds the model calls of one run (20 by default, 50 at most).
- A **viewer** agent gets read tools only. To give it a write tool, create it with the member role.
- Start a run like any agent run (button, or the `agent_run` flow node): the text of its input files is its task, the activated skills it pins are its instructions.
- By default a write never runs on its own. Each write call of the agent becomes a proposal of kind **Agent action** in **To validate**, with the tool and its exact arguments. **Accept** runs those arguments with *your* rights, checked again (role, organization, version read); **Reject** keeps nothing. An agent action cannot be corrected: reject it and start a new run.
- **Who decides**: next to each ticked write tool, choose **A person** (the default), **Validator agent**, **Validator, then a person**, or **Runs on its own**. A write that runs on its own, or that the validator accepts, runs with the agent's own service account (its roles in Access), and stays listed in **To validate** as decided. After the agent read outside content (a mail, a web page, a document), a write set to run on its own waits for a person instead.
- **Validator agent**: another internal agent of the organization, picked under **Validator agent**. It reads each request as data (never as instructions) and answers accept or reject; it cannot change the request. With **Validator, then a person**, an accepted request still waits for you, marked "Accepted by the validator agent". If the validator does not answer clearly, or its run fails, the request waits for a person.
- Some tools are always decided by a person (access, secrets, markings, devices, deletions): their choice is locked to **A person**.
- **A validator that decides too much stops itself**: past 30 decisions within an hour, or 20 acceptances in a row without a refusal, its requests go to people. Its record says why; **Rearm** lets it decide again.
- The run's detail shows the engine (**Internal**) and a link to the proposals of this agent. Its final answer is the **report** output, to download.

## Troubleshooting

- **The run failed with agent-loop-detected**: the agent called the same tool with the same arguments three times. Make its input clearer, or its skill, then start a new run.
- **Accept answers proposal-tool-failed or another refusal**: the tool refused with your rights (missing permission, object changed since the agent read it). The proposal stays open; fix the cause in the page concerned, or reject it.
- **The run failed with agent-run-llm-budget or agent-gateway-quota**: its model calls, or the organization's daily calls of agents, are spent. Raise the agent's budget in its record, or wait for the next day.
- **The run failed with agent-gateway-not-configured**: no model provider serves the agents of the organization. Add one in the AI settings, for the agent usage, or pick a provider on the agent.
- **A write tool cannot be ticked**: the agent is a viewer.
- **Saving answers that an agent cannot validate its own requests, or that the validator must be an internal agent**: pick another internal agent under **Validator agent**.
- **Requests set to a validator wait for a person**: the validator tripped (its record shows the reason and **Rearm**), it is part of the same chain of runs as the asking agent, or its run failed. Decide them in **To validate**, or fix the validator and rearm it.
