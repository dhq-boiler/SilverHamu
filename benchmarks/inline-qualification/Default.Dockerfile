FROM catdb-capacity-gate AS checks
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benchmarks ./benchmarks
RUN cargo test --locked --release --features service -- --test-threads=1
RUN cargo build --locked --release --features service --lib --bin catdb-server
RUN cargo build --locked --release --no-default-features --features service,inline-latest --lib --target-dir target/rollback
FROM catdb-inline-qualification:safety
COPY --from=checks /build/target/release/libnewdbms.so /default.so
COPY --from=checks /build/target/release/catdb-server /default-server
COPY --from=checks /build/target/rollback/release/libnewdbms.so /rollback.so
ENV CATDB_SERVER=/default-server NEWDBMS_TEST_DLL=/default.so
RUN cmp /default.so /usr/local/lib/libnewdbms.so && cmp /default-server /usr/local/bin/catdb-server
RUN python -m unittest discover -s tests -p '*ffi_checks.py'
RUN python tests/service_protocol.py
RUN python tests/key_storage_transition.py /rollback.so /default.so
