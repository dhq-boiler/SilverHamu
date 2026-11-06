FROM rust:1.89-bookworm AS observer
COPY trace.c /tmp/trace.c
RUN cc -shared -fPIC -O2 -Wall -Wextra -Werror -o /tmp/libiotrace.so /tmp/trace.c -ldl
FROM catdb-solo:local
COPY --from=observer /tmp/libiotrace.so /usr/local/lib/libiotrace.so
COPY client.py /io/client.py
