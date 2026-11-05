FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG CATDB_FEATURES=service,lock-timing
RUN cargo build --locked --release --features "$CATDB_FEATURES" --lib --bin catdb-server --bin reclamation-bench
FROM python:3.12-slim-bookworm
COPY --from=build /build/target/release/libnewdbms.so /usr/local/lib/
COPY --from=build /build/target/release/catdb-server /usr/local/bin/
COPY --from=build /build/target/release/reclamation-bench /usr/local/bin/
CMD ["sleep", "infinity"]
