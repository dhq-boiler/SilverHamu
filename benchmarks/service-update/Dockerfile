FROM rust:1.89-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG CATDB_FEATURES=service
RUN cargo build --locked --release --features "$CATDB_FEATURES" --lib --bin silverhamu-server
FROM python:3.12-slim-bookworm
RUN pip install --no-cache-dir psycopg[binary]==3.2.10 PyMySQL==1.1.2 cryptography==46.0.3 oracledb==3.4.1 matplotlib==3.10.7
COPY --from=build /build/target/release/libnewdbms.so /usr/local/lib/
COPY --from=build /build/target/release/silverhamu-server /usr/local/bin/
COPY benchmarks /bench
COPY scripts/silverhamu-client.py /bench/catdb_client.py
WORKDIR /bench
ENV PYTHONUNBUFFERED=1
CMD ["sleep", "infinity"]
