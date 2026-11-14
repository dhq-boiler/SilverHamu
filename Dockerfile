# syntax=docker/dockerfile:1
FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG CATDB_FEATURES=service
RUN cargo build --locked --release --features "$CATDB_FEATURES" --bin silverhamu-server --bin silverhamu

FROM debian:bookworm-slim AS runtime
RUN groupadd --gid 10001 silverhamu && useradd --uid 10001 --gid silverhamu --no-create-home silverhamu \
    && mkdir /data && chown silverhamu:silverhamu /data
COPY --from=build /build/target/release/silverhamu-server /usr/local/bin/silverhamu-server
COPY --from=build /build/target/release/silverhamu /usr/local/bin/silverhamu
COPY Cargo.lock /usr/share/silverhamu/Cargo.lock
USER 10001:10001
WORKDIR /data
VOLUME ["/data"]
EXPOSE 7400
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["silverhamu-server", "--healthcheck", "--token-file", "/run/secrets/silverhamu_token"]
ENTRYPOINT ["silverhamu-server"]
CMD ["--db", "/data/state.ndb", "--init", "--bind", "0.0.0.0:7400", "--allow-remote", "--token-file", "/run/secrets/silverhamu_token"]
