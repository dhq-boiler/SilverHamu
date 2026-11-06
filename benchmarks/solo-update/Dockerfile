FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --features service --lib --bin solo-update-bench && cp target/release/solo-update-bench /tmp/plain && cp target/release/libnewdbms.so /tmp/libnewdbms.so
RUN cargo build --locked --release --features service,lock-timing --bin solo-update-bench
FROM catdb-peer-20261107:local
COPY --from=build /tmp/plain /usr/local/bin/solo-update-bench
COPY --from=build /build/target/release/solo-update-bench /usr/local/bin/solo-update-diag
COPY --from=build /tmp/libnewdbms.so /usr/local/lib/libnewdbms.so
COPY benchmarks /bench
