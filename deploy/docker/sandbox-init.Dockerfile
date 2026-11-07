# Image of the init steps of an agent sandbox (agent-environments.md D714): git for the clone,
# curl for the egress gate (SEC-E7) and the artifact transfers.
FROM cgr.dev/chainguard/wolfi-base@sha256:771146c2dfc880902a185f060c3db5571473c46b267edd98c3119c83068dfb61
RUN apk add --no-cache git curl
USER 65532:65532
ENV HOME=/tmp
