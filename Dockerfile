FROM rust:1.85-bookworm AS builder

WORKDIR /app

# Install protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY proto/ proto/
COPY crates/ crates/

# Build release
RUN cargo build --release -p valka-server -p valka-cli

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/valka-server /usr/local/bin/valka-server
COPY --from=builder /app/target/release/valka /usr/local/bin/valka

EXPOSE 50051 8080

ENTRYPOINT ["valka-server"]
