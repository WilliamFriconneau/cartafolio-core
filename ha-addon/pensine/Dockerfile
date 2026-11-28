ARG BUILD_FROM
FROM ${BUILD_FROM}
ARG BUILD_ARCH
ARG PENSINE_VERSION

RUN apk add --no-cache tzdata curl jq tailscale \
 && case "${BUILD_ARCH}" in \
      amd64) asset=pensine-linux-x64 ;; \
      aarch64) asset=pensine-linux-arm64 ;; \
      *) echo "Unsupported architecture: ${BUILD_ARCH}" >&2; exit 1 ;; \
    esac \
 && base="https://github.com/WilliamFriconneau/pensine-core/releases/download/v${PENSINE_VERSION}" \
 && cd /tmp \
 && curl -fsSLO "${base}/SHA256SUMS" \
 && curl -fsSLO "${base}/${asset}" \
 && awk -v asset="$asset" '$2 == asset' SHA256SUMS | sha256sum -c - \
 && install -m 0755 "$asset" /usr/bin/pensine \
 && rm "$asset" SHA256SUMS

COPY run.sh /run.sh
CMD ["/run.sh"]
