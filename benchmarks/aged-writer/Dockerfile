FROM rust:1.89-bookworm
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benchmarks ./benchmarks
RUN cargo test --locked --release --lib scaled_gate -- --nocapture --test-threads=1

RUN cargo test --locked --release --features fifo-writer --lib -- --test-threads=1

RUN cargo test --locked --release --features aged-writer --lib -- --test-threads=1
