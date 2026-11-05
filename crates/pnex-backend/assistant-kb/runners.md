---
id: runners
title: Runners (where hosted work runs)
kind: feature
pages: 
nodes: 
err_codes: runner-join-token-invalid, runner-unauthorized, runner-not-found, runner-job-not-held
tools: 
tags: runner, job, pod, cluster, kubernetes, join token, capability, sandbox
---
A runner is a small program an organization installs in its own Kubernetes namespace. It connects out to PneX, takes jobs of its organization, and runs each one in a hardened pod that it destroys afterwards. PneX keeps no cluster credentials.

There is no page for runners yet: they are managed through the API.

## Enrolling
- An Admin creates a join token. It works once and lasts one hour; it is shown once.
- The runner consumes it when it starts, then keeps its own token in its namespace.
- Revoking a runner stops its token at once, and the jobs it held go back to the queue.

## Jobs
- A job runs an image pinned by digest, never by tag.
- A job can require capabilities, such as `isolation:pod`. Only a runner announcing all of them takes it. A runner without a hardened runtime announces `isolation:none`.
- A job nobody takes within an hour ends as `expired`: it is never moved elsewhere silently.
- If a runner stops answering, its job goes back to the queue after two minutes, up to three attempts, then fails with the outcome `lease_expired`.

## Limits
- You can read runners and jobs, but you cannot create tokens, queue jobs or revoke runners: those stay with the people who hold `runners:manage` and `runners:delete`.
