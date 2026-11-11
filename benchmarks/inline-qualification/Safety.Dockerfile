FROM silverhamu-capacity-gate AS checks
COPY Cargo.lock ./
COPY benchmarks/inline-qualification/measured-Cargo.toml ./Cargo.toml
COPY src ./src
COPY benchmarks ./benchmarks
RUN cargo test --locked --release --features service,inline-key -- --test-threads=1
RUN cargo test --locked --release --features service --lib -- --test-threads=1
FROM silverhamu-inline-qualification:baseline AS baseline
FROM silverhamu-inline-qualification:inline
WORKDIR /validation
COPY tests ./tests
COPY scripts ./scripts
COPY --from=baseline /usr/local/lib/libnewdbms.so /baseline.so
COPY --from=checks /build/Cargo.toml /checked-Cargo.toml
ENV CATDB_SERVER=/usr/local/bin/silverhamu-server NEWDBMS_TEST_DLL=/usr/local/lib/libnewdbms.so
RUN python -m unittest discover -s tests -p '*ffi_checks.py'
RUN python tests/service_protocol.py
RUN python tests/key_storage_transition.py /baseline.so /usr/local/lib/libnewdbms.so
