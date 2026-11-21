ARG BUILD_FROM
FROM ${BUILD_FROM}
ARG BUILD_ARCH
# Set in build.yaml, and kept equal to `version` in config.yaml (the CI checks it).
ARG MEMCP_VERSION
# The static (musl) programs of the release of this version, checked against the SHA256SUMS published with it:
# the manager (which reads the voice notes and watches the folders itself when that is turned on), the engine (whose
# commands work on one person's directory), the optional janitor and the program that keeps nodes in step.
# curl stays: the base image's own tools (bashio, which reads the options) need it.
RUN apk add --no-cache tzdata curl poppler-utils \
 && case "${BUILD_ARCH}" in \
      amd64) T=x86_64-unknown-linux-musl ;; \
      aarch64) T=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture ${BUILD_ARCH}" >&2; exit 1 ;; \
    esac \
 && base="https://github.com/WilliamFriconneau/memcp-core/releases/download/v${MEMCP_VERSION}" \
 && cd /tmp && curl -fsSLO "${base}/SHA256SUMS" \
 && for b in memcp memcp-manager memcp-janitor memcp-sync; do \
      f="${b}-v${MEMCP_VERSION}-${T}"; \
      curl -fsSLO "${base}/${f}"; \
      grep " ${f}\$" SHA256SUMS | sha256sum -c -; \
      install -m 0755 "${f}" "/usr/bin/${b}"; \
    done \
 && for b in memcp memcp-manager memcp-sync; do ln -s "${b}" "/usr/bin/cartafolio${b#memcp}"; done \
 && rm -f /tmp/*
# The library reads the text of a PDF with this program (a scanned book needs an OCR: see docs/LIBRARY.md).
ENV MEMCP_PDF_TEXT_COMMAND="pdftotext -layout {} -"
COPY run.sh /run.sh
RUN chmod a+x /run.sh
CMD [ "/run.sh" ]
