FROM rust:1.93-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY rs-peer-workspace-shared/Cargo.toml rs-peer-workspace-shared/Cargo.toml
COPY rs-peer-workspace-shared/src rs-peer-workspace-shared/src
COPY rs-peer-workspace-proxy/Cargo.toml rs-peer-workspace-proxy/Cargo.toml
COPY rs-peer-workspace-proxy/src rs-peer-workspace-proxy/src

RUN cargo build --release -p rs-peer-workspace-proxy

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/rs-peer-workspace-proxy /usr/local/bin/rs-peer-workspace-proxy

EXPOSE 9000

ENTRYPOINT ["/usr/local/bin/rs-peer-workspace-proxy"]
