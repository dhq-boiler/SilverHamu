FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --features service,history-metrics,allocation-metrics --bin history-capacity-bench && cp target/release/history-capacity-bench /tmp/probe-counted
RUN cargo build --locked --release --features service,history-metrics --bin history-capacity-bench && cp target/release/history-capacity-bench /tmp/probe-plain
FROM python:3.12-slim-bookworm
COPY --from=build /tmp/probe-counted /usr/local/bin/probe-counted
COPY --from=build /tmp/probe-plain /usr/local/bin/probe-plain
