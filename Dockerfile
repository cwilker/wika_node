# Builds wika-node from this checkout.
#   docker build -t wika-node .
#   docker run --rm -p 9944:9944 wika-node --dev --rpc-external --rpc-cors=all
FROM docker.io/library/rust:1-bookworm AS builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends clang libclang-dev protobuf-compiler \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32v1-none && rustup component add rust-src
WORKDIR /wika_node
COPY . .
RUN cargo build --locked --release -p wika-node

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 1000 -U -s /bin/sh -d /wika wika \
 && mkdir -p /data && chown wika:wika /data
COPY --from=builder /wika_node/target/release/wika-node /usr/local/bin/wika-node
USER wika
EXPOSE 30333 9944 9615
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/wika-node", "--base-path", "/data"]
