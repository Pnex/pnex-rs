---
id: environments
title: Agent environments (the image an agent runs in)
kind: feature
pages: 
nodes: 
err_codes: environment-not-found, environment-version-not-found, environment-name-taken, environment-version-conflict, environment-secret-in-dockerfile, image-registry-in-use, environment-build-not-found, environment-build-unavailable, environment-build-no-write-secret, environment-build-running, environment-scan-not-found, environment-sbom-unavailable, environment-build-not-ready
tools: 
tags: environment, dockerfile, image, version, build, agent, context
---
An environment describes the image an agent runs in. Its source lives in PneX: a Dockerfile, its text context files and its build arguments, with no git repository involved. It also names the image registry its images go to.

There is no page for environments yet: they are managed through the API.

## Versions
- Each save creates a new numbered version, which never changes afterwards. Its fingerprint is the SHA-256 of its source.
- A save names the latest version it was based on. If someone saved in between, it is refused rather than overwriting their work.
- The source is limited to 1 MB in all, text only. A binary comes from the base image, or from a download pinned by checksum in a `RUN` step.

## Builds
An Admin starts the build of a version. It runs in a pod created by a runner of the organization, whose cluster must allow it. The image goes to the registry of the environment, and the build records its digest: a run executes that digest, never a tag. One build runs at a time per environment. A build nobody takes within an hour fails.

Builds reuse what earlier builds of the same environment produced, when the deployment has a cache store: a second build of an unchanged Dockerfile is much shorter. Each environment has its own cache, and a build can reach no other.

## Activating
An Admin chooses the active build: the image the runs of this environment execute. Going back means activating an earlier build. Only a build that produced an image can be activated. A scan result does not block it for now.

## Scans
When the deployment has a scanner, each successful build is scanned for known vulnerabilities. The build shows the count per severity, and its scan lists each finding with the version that fixes it, when one exists. A rebuild often picks up that fix through a newer base image. The active build is scanned again every day, since a flaw is often published after the build. A failed scan leaves the build usable. The scan sees the image only: not what a setup step installs later, nor the code of the project.

## SBOM and ontology
Each scan produces an SBOM, the list of what the image contains, in the CycloneDX format. The document can be downloaded from the build, and the follow-up of an environment lists each of its scans with its counts per severity. The scan also fills the ontology, with the Software supply pack installed on first use: the image, its SBOM, its components and the vulnerabilities it is exposed to. A component is shared by every image that holds it, so a query can answer where a package runs. A vulnerability a later scan no longer finds closes its link, which stays in the history.

## Limits
- Reading takes `environments:read`. Saving takes `environments:write`, held by Members. Deleting takes `environments:delete`, held by Admins.
- A registry used by an environment cannot be deleted.

## Troubleshooting
- **"This deployment has no builder image"**: the platform administrator has not configured builds.
- **"The registry has no write secret"**: add a write secret to the registry of this environment.
- **"A build is not finished"**: wait for it to end, then start the next one.
- **"This source holds what looks like a credential"**: the message names the file or build argument. Remove the value. A build argument stays readable in the image history, so it is no place for a secret either.
- **"This environment changed since you opened it"**: reload it, then save your changes again.
- **"Registry used by an environment"**: point that environment to another registry, or delete it, then delete the registry.
