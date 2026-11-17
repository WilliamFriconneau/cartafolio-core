ARG BUILD_FROM
FROM ${BUILD_FROM}
ARG BUILD_ARCH
# Set in build.yaml, and kept equal to `version` in config.yaml (the CI checks it).
ARG MEMCP_VERSION
# The static (musl) programs of the release of this version, checked against the SHA256SUMS published with it:
# the manager, the engine (whose commands work on one person's directory), the optional janitor and the bridge
# (which reads voice notes by a speech service when that is turned on).
# curl stays: the base image's own tools (bashio, which reads the options) need it.
RUN apk add --no-cache tzdata curl \
 && case "${BUILD_ARCH}" in \
      amd64) T=x86_64-unknown-linux-musl ;; \
      aarch64) T=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture ${BUILD_ARCH}" >&2; exit 1 ;; \
    esac \
 && base="https://github.com/WilliamFriconneau/memcp-core/releases/download/v${MEMCP_VERSION}" \
 && cd /tmp && curl -fsSLO "${base}/SHA256SUMS" \
 && for b in memcp memcp-manager memcp-janitor memcp-bridge memcp-sync; do \
      f="${b}-v${MEMCP_VERSION}-${T}"; \
      curl -fsSLO "${base}/${f}"; \
      grep " ${f}\$" SHA256SUMS | sha256sum -c -; \
      install -m 0755 "${f}" "/usr/bin/${b}"; \
    done \
 && rm -f /tmp/*
COPY run.sh /run.sh
RUN chmod a+x /run.sh
CMD [ "/run.sh" ]
