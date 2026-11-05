# Image of the pod that scans an environment image (agent-environments.md D720): Trivy from
# the Wolfi package, never the vendor's releases, and pinned: its releases 0.69.4 to 0.69.6
# were compromised in March 2026 (CVE-2026-33634). Moving this pin is a reviewed change.
FROM cgr.dev/chainguard/wolfi-base@sha256:771146c2dfc880902a185f060c3db5571473c46b267edd98c3119c83068dfb61
RUN apk add --no-cache trivy=0.68.2-r1
USER 65532:65532
ENV HOME=/tmp TRIVY_CACHE_DIR=/tmp/trivy TRIVY_NO_PROGRESS=true
ENTRYPOINT ["/usr/bin/trivy"]
