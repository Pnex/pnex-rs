# Image of the clone step of an agent sandbox (agent-environments.md D714): git, nothing else.
FROM cgr.dev/chainguard/wolfi-base@sha256:771146c2dfc880902a185f060c3db5571473c46b267edd98c3119c83068dfb61
RUN apk add --no-cache git
USER 65532:65532
ENV HOME=/tmp
