FROM rust:1.94.0-bookworm AS builder
WORKDIR /app

# The base image and repository override intentionally use the same pinned
# toolchain. This image is a source-build convenience, not a published image.
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
RUN cargo fetch --locked
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/kite /usr/local/bin/kite
ENTRYPOINT ["kite"]
CMD ["--help"]
