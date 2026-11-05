# Image of the pod that builds an agent environment (agent-environments.md D700, D701):
# rootless BuildKit on Wolfi. Not the firmware `builder` target of the main Dockerfile.
FROM cgr.dev/chainguard/wolfi-base@sha256:771146c2dfc880902a185f060c3db5571473c46b267edd98c3119c83068dfb61
RUN apk add --no-cache buildctl buildkitd rootlesskit runc shadow-subids slirp4netns \
 && chmod u+s /usr/bin/newuidmap /usr/bin/newgidmap \
 && adduser -D -u 1000 user \
 && echo 'user:100000:65536' > /etc/subuid \
 && echo 'user:100000:65536' > /etc/subgid \
 && mkdir -p /run/user/1000 /run/buildkit /home/user/.local/share/buildkit \
 && chown -R user:user /run/user/1000 /run/buildkit /home/user \
 && chmod 700 /run/user/1000
COPY <<'SCRIPT' /usr/bin/pnex-image-build
#!/bin/sh
# Builds with the given buildctl arguments, then hands the digest to the runner
# through the termination message of the container.
set -eu
buildctl-daemonless.sh build --metadata-file /tmp/build-metadata.json "$@"
sed -n 's/.*"containerimage.digest": *"\(sha256:[0-9a-f]\{64\}\)".*/\1/p' \
  /tmp/build-metadata.json > /dev/termination-log
SCRIPT
RUN chmod 755 /usr/bin/pnex-image-build
USER 1000:1000
ENV HOME=/home/user XDG_RUNTIME_DIR=/run/user/1000 XDG_DATA_HOME=/home/user/.local/share \
    BUILDKITD_FLAGS="--oci-worker-no-process-sandbox --root=/home/user/.local/share/buildkit"
ENTRYPOINT ["/usr/bin/pnex-image-build"]
