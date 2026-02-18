# Stage 1: Build WebUI
FROM oven/bun:1.3 AS web-builder

WORKDIR /app/web
COPY web/package.json web/bun.lock* web/package-lock.json* ./
RUN bun install
COPY web/ ./
RUN bun run build

# Stage 2: Build Rust binaries
FROM rust:1.88-bookworm AS builder

WORKDIR /app

# Install protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Copy workspace manifest + lock first (for dependency caching)
COPY Cargo.toml Cargo.lock ./
COPY proto/ proto/
COPY crates/ crates/
COPY examples/ examples/

# Build release with BuildKit cache mounts for cargo registry + target
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release -p valka-server -p valka-cli \
    && cp target/release/valka-server target/release/valka /tmp/

# Stage 3: Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/valka-server /usr/local/bin/valka-server
COPY --from=builder /tmp/valka /usr/local/bin/valka
COPY --from=web-builder /app/web/dist /usr/share/valka/web

RUN useradd --system --uid 1001 --no-create-home valka

ENV VALKA_WEB_DIR=/usr/share/valka/web

EXPOSE 50051 8989 7280/udp

HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8989/healthz || exit 1

USER valka

ENTRYPOINT ["valka-server"]
