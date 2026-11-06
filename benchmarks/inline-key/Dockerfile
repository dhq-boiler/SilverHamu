FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG EXTRA_FEATURES=""
RUN cargo build --locked --release --features service${EXTRA_FEATURES} --lib --bin catdb-server && cp target/release/catdb-server /tmp/server && cp target/release/libnewdbms.so /tmp/libnewdbms.so
RUN cargo build --locked --release --features service,history-metrics,allocation-metrics${EXTRA_FEATURES} --bin key-allocation-bench
FROM catdb-comparison:local
COPY benchmarks /bench
COPY scripts/catdb-client.py /bench/catdb_client.py
COPY --from=build /tmp/libnewdbms.so /usr/local/lib/libnewdbms.so
COPY --from=build /tmp/server /usr/local/bin/catdb-server
COPY --from=build /build/target/release/key-allocation-bench /usr/local/bin/key-allocation-bench
