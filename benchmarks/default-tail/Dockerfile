ARG BUILDER_IMAGE=rust:1.89-bookworm
ARG RUNTIME_IMAGE=catdb-inline-default:default
FROM ${BUILDER_IMAGE} AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG FEATURE_FLAGS=""
ARG EXTRA_FEATURES=""
RUN mkdir -p /record && rustc -vV > /record/rustc.txt && cargo -vV > /record/cargo.txt \
    && cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-unknown-linux-gnu ${FEATURE_FLAGS} --features service${EXTRA_FEATURES} > /record/metadata.json \
    && cargo build --locked --offline --release --target-dir /matched ${FEATURE_FLAGS} --features service${EXTRA_FEATURES} --lib --bin silverhamu-server > /record/build.stdout 2> /record/build.stderr \
    && sha256sum /matched/release/silverhamu-server /matched/release/libnewdbms.so > /record/binaries.sha256 \
    && objcopy --dump-section .text=/record/server.text /matched/release/silverhamu-server \
    && sha256sum /record/server.text > /record/text.sha256
FROM ${RUNTIME_IMAGE}
COPY --from=build /matched/release/silverhamu-server /usr/local/bin/
COPY --from=build /matched/release/libnewdbms.so /usr/local/lib/
COPY --from=build /record /build-record
