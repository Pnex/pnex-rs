---
id: agent-runs
title: Agent runs (hosted agents and their model gateway)
kind: feature
pages: 
nodes: 
err_codes: agent-run-not-found, agent-run-no-active-build, agent-run-ended, agent-run-unauthorized, agent-run-llm-budget, agent-gateway-quota, agent-gateway-not-configured, agent-gateway-protocol-mismatch, agent-gateway-invalid, agent-gateway-upstream
tools: 
tags: agent, run, sandbox, gateway, llm, token, budget, environment
---
A run starts a hosted agent in a sandbox: one pod, on a runner of the organization, from the active build of an environment. It ends with the pod; nothing stays running between runs.

There is no page for runs yet: they are started through the API.

## The model
- The agent reaches the model of the organization through the PneX gateway only. The provider key stays in the vault and never enters the sandbox.
- The organization picks that model in the AI settings, under the usage "Hosted agents". Whatever model the agent asks for, the gateway uses this one.
- The run gets a run token instead of a key. It works only while the run is running: a stop or the end of the run closes it at once.

## Limits
- Each run has a number of model calls (200 by default); past it, calls are refused with `agent-run-llm-budget`.
- The agents of an organization also share a number of calls per day, apart from the assistant and the flows.
- Tokens and calls of each run are counted and shown on the run.

## When something goes wrong
- `agent-run-no-active-build`: the environment has no build to run. Build it, then activate the build on its environment.
- `agent-gateway-not-configured`: no model provider is set for the organization. Add one in the AI settings and route the usage "Hosted agents" to it.
- `agent-gateway-protocol-mismatch`: the engine of the agent speaks another protocol than the provider (Anthropic or OpenAI compatible). Pick a provider of the same kind.

## What you can do
- You can read runs and their usage. You never start or stop a run: those stay with the people who hold `agents:run` and `agents:stop`.
