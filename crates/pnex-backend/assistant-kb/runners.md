---
id: runners
title: Runners (where hosted work runs)
kind: feature
pages: /agents
nodes: 
err_codes: runner-join-token-invalid, runner-unauthorized, runner-not-found, runner-job-not-held
tools: 
tags: runner, job, pod, cluster, kubernetes, join token, capability, sandbox
---
A runner is a small program an organization installs in its own Kubernetes namespace. It connects out to PneX, takes jobs of its organization, and runs each one in a hardened pod that it destroys afterwards. PneX keeps no cluster credentials.

They are listed in the Agents page, Runners tab, with their state (online or offline) and their capabilities.

## Enrolling
- An Admin clicks **New join token** in the Runners tab and names the runner. The token works once and lasts one hour; it is shown once, with a **Copy** button and the commands that install the runner: a namespace, the Secret `pnex-runner-join` that holds the token, then the `pnex-runner` chart of the pnex-deploy repository pointed to this server. **Copy the commands** copies them with the token filled in.
- The runner consumes it when it starts, then keeps its own token in its namespace.
- **Revoke**, then **Confirm revocation**, on a runner stops its token at once, and the jobs it held go back to the queue.

## Jobs
- A job runs an image pinned by digest, never by tag.
- A job can require capabilities, such as `isolation:pod`. Only a runner announcing all of them takes it. A runner without a hardened runtime announces `isolation:none`.
- A job nobody takes within an hour ends as `expired`: it is never moved elsewhere silently.
- If a runner stops answering, its job goes back to the queue after two minutes, up to three attempts, then fails with the outcome `lease_expired`.

## Limits
- You can read runners and jobs, but you cannot create tokens, queue jobs or revoke runners: those stay with the people who hold `runners:manage` and `runners:delete`.
