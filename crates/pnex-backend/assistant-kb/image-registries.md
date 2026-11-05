---
id: image-registries
title: Image registries (where agent images live)
kind: feature
pages: 
nodes: 
err_codes: image-registry-not-found, image-registry-name-taken, image-registry-version-conflict, image-registry-unreachable, image-registry-host-refused, image-registry-auth-failed, image-registry-access-denied, image-registry-in-use
tools: 
tags: registry, docker, image, oci, repository, pull, push, robot account
---
An image registry saves the access to an OCI registry and one repository: any registry following the distribution API works. It holds two distinct secrets in the vault: a read secret, which only pulls, and a write secret, which only the image build uses. One secret cannot fill both roles, so that what runs an image can never push one.

There is no page for image registries yet: they are managed through the API.

## Testing
A test tries the pull right with the read secret, then the push right with the write secret when one is set, by opening an upload and cancelling it. No image moves. A repository that does not exist yet passes the pull test: it appears at the first push.

## Limits
- You can read the registries, but not create, change, test or delete them: that takes the `image-registries` permissions, held by Admins.
- A typed secret needs the right to write secrets. Changing the host or the TLS setting asks for the secrets again.

## Troubleshooting
- **"Login refused by this registry"**: check the user and secret of the failing check.
- **"This login may not use this repository"**: the login works, but has no right on this repository: give it the right, or fix the repository name.
- **"No answer from this registry"**: check the host and port, and that TLS matches what the registry serves.
- **"This address cannot be reached"**: it is private, or TLS is off: ask the platform administrator to allow that host.
