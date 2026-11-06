---
id: agent-runs
title: Agent runs (hosted agents and their model gateway)
kind: feature
pages: /agents
nodes: 
err_codes: agent-run-not-found, agent-run-no-active-build, agent-run-ended, agent-run-unauthorized, agent-run-llm-budget, agent-gateway-quota, agent-gateway-not-configured, agent-gateway-protocol-mismatch, agent-gateway-invalid, agent-gateway-upstream, agent-run-logs-unavailable, agent-artifact-not-found, agent-artifact-not-declared, agent-artifact-already-sent, agent-artifact-too-large, agent-artifact-interrupted, agent-artifact-storage-unavailable, agent-run-output-missing, agent-not-found, agent-name-taken, agent-version-conflict, agent-quota, agent-provider-not-found
tools: 
tags: agent, run, sandbox, gateway, llm, token, budget, environment, logs, journal, artifact, file, input, output
---
A run starts a hosted agent in a sandbox: one pod, on a runner of the organization, from the active build of an environment. It ends with the pod; nothing stays running between runs.

The Agents page lists the agents and their runs. From it a person creates an agent, starts a run by giving the files it asks for, follows its log, downloads its outputs and stops it.

## Agents
- An agent is an identity of the organization: a service account, listed with the other service accounts, never a member. Only an admin creates or changes one.
- It is born viewer and can be raised to member at most, never above the person who sets it.
- Its record says what it runs: environment, command, the files it needs, the files it leaves, its limits, and optionally a model provider of its own with its own key.
- Every change of the record is a new version. A run starts from the current version and may only ask for less: a shorter time, fewer model calls. It must be given every file the record names.
- The tier of the organization caps how many agents it can have, apart from its members (`agent-quota`).
- Deleting an agent removes its identity, keys and roles; its past runs stay readable.

## The model
- The agent reaches the model of the organization through the PneX gateway only. The provider key stays in the vault and never enters the sandbox.
- The organization picks that model in the AI settings, under the usage "Hosted agents". Whatever model the agent asks for, the gateway uses this one.
- The run gets a run token instead of a key. It works only while the run is running: a stop or the end of the run closes it at once.

## The journal of a run
- When a run ends, its runner sends the last lines of each container of the sandbox: clone, test services, setup, then the agent, in that order.
- The secrets of the run are masked, and so is anything shaped like a known credential. A container with more output keeps its last lines only, marked as such.
- The journal is for debugging. A run stopped by hand keeps it too: its runner sends it right after the stop.
- `agent-run-logs-unavailable`: the journal cannot be read on this deployment right now. Try again later; the run itself is not affected.

## Files in and out of a run (artifacts)
- A run can take files as inputs and leave files as outputs, by name. An input is a file someone uploaded, or an output of an earlier run.
- Inputs are placed before the agent starts and checked against their fingerprint; a changed file stops the run before anything runs.
- Outputs are collected when the agent ends, also when it failed. An output marked required that is not there fails the run (`agent-run-output-missing`).
- A file over the size limit is refused whole, never cut (`agent-artifact-too-large`).
- Files expire after a while and are then removed. Their content is only downloaded, never shown, and needs its own right: it may hold a secret the agent wrote.
- The agent never talks to the storage itself: PneX carries every file.

## Repositories and test services
- A run, or an agent's record, can name repositories to clone (`https` only) and test services to start beside the agent, such as a database.
- A repository's read token is a secret of the vault: only the clone step gets it, never the agent.
- A service variable set to `@generated` becomes a fresh password at every run; the agent reads it as `<SERVICE>_<VARIABLE>`. No password is ever written in the record.

## Dependency caches
- A run, or an agent's record, can keep folders from one run to the next, such as the downloaded packages of a build tool.
- Each cache is keyed by its lock files: the same lock file restores the same cache; a changed one starts from the latest cache and saves a new one.
- Caches belong to their environment, expire after two weeks without use, and have a size limit.

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
