# syntax=docker/dockerfile:1
FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --features service --bin catdb-server

FROM debian:bookworm-slim AS runtime
RUN groupadd --gid 10001 catdb && useradd --uid 10001 --gid catdb --no-create-home catdb \
    && mkdir /data && chown catdb:catdb /data
COPY --from=build /build/target/release/catdb-server /usr/local/bin/catdb-server
USER 10001:10001
WORKDIR /data
VOLUME ["/data"]
EXPOSE 7400
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["catdb-server", "--healthcheck", "--token-file", "/run/secrets/catdb_token"]
ENTRYPOINT ["catdb-server"]
CMD ["--db", "/data/state.ndb", "--init", "--bind", "0.0.0.0:7400", "--allow-remote", "--token-file", "/run/secrets/catdb_token"]
