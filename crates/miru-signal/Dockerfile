FROM rust:1.80-slim AS builder
WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY crates/miru-common/Cargo.toml crates/miru-common/
COPY crates/miru-signal/Cargo.toml crates/miru-signal/
RUN mkdir -p crates/miru-common/src && echo "pub fn dummy() {}" > crates/miru-common/src/lib.rs
RUN mkdir -p crates/miru-signal/src && echo "fn main() {}" > crates/miru-signal/src/main.rs
RUN cargo build --release -p miru-signal 2>/dev/null || true

# Build actual source
COPY . .
RUN touch crates/miru-common/src/lib.rs crates/miru-signal/src/main.rs
RUN cargo build --release -p miru-signal

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/miru-signal /usr/local/bin/

EXPOSE 21115 21117

ENV MIRU_LOG=info
ENV MIRU_PUBLIC_HOST=localhost

HEALTHCHECK --interval=30s --timeout=5s \
    CMD wget -qO- http://localhost:21115/health || exit 1

CMD ["miru-signal"]
