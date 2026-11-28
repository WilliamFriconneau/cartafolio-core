ARG BUILD_FROM=rust:1.100-slim
FROM ${BUILD_FROM} AS builder

WORKDIR /build
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked --bin pensine

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates jq \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data pensine

COPY --from=builder /build/target/release/pensine /usr/local/bin/pensine
COPY ha-addon/run.sh /run.sh
RUN chmod +x /run.sh

WORKDIR /data
USER pensine
EXPOSE 8791 8099
ENTRYPOINT ["/run.sh"]
