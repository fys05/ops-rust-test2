FROM rust:1.98-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY public ./public
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/ops-rust-test2 /usr/local/bin/ops-rust-test2
COPY public ./public
ENV BIND_ADDR=0.0.0.0:8080
ENV DATABASE_URL=sqlite:/data/class_manager.db
RUN mkdir -p /data
VOLUME ["/data"]
EXPOSE 8080
CMD ["ops-rust-test2"]
