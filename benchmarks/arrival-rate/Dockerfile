FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG CATDB_FEATURES=service
RUN cargo build --locked --release --features "$CATDB_FEATURES" --lib --bin catdb-server
FROM catdb-comparison:local
COPY --from=build /build/target/release/libnewdbms.so /usr/local/lib/
COPY --from=build /build/target/release/catdb-server /usr/local/bin/
