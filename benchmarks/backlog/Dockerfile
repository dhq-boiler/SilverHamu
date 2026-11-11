FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG CATDB_FEATURES=service
RUN cargo build --locked --release --features "$CATDB_FEATURES" --lib --bin silverhamu-server
FROM catdb-comparison:local
COPY --from=build /build/target/release/libsilverhamu.so /usr/local/lib/
COPY --from=build /build/target/release/silverhamu-server /usr/local/bin/
