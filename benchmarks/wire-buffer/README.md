# Optional framed-JSON buffering experiment

Only connection-local framing changes. `wire-buffer` keeps at most 4096 bytes of
receive capacity and 4096 bytes of transmit capacity per connection. Larger
buffers are temporary and discarded after success/error. With the existing 256
connection maximum, retained buffer payload capacity is at most 2MiB total,
separate from engine logical resource accounting and inside process RSS.

Response prefix and JSON body are serialized into one bounded frame before any
socket write; partial writes, Interrupted/WouldBlock/TimedOut retry, absolute
request deadlines and stop checks remain. Receive reads exactly the prefix and
body, never consuming bytes from the next frame. Protocol/authentication/transaction
semantics and durability are unchanged. No binary protocol or inline-key feature.

Build baseline and buffered images using this Dockerfile and EXTRA_FEATURES=,wire-buffer
only for buffered. Both include matching current embedded library and service.
run.py --out NEW/raw runs three alternating 1M/256B/1CPU/512MiB comparisons,
8000 read +8000 mixed requests per trial, 1000 arrivals/s, old reader retained.
Run report.py NEW for per-trial graphs. All request timings include producer and
client queue delay. Throughput at the fixed offered rate is not maximum capacity.

wire-probe is a separate allocation-instrumented executable. It warms reusable
buffers, then performs10000 parse/serialize operations with the exact service Wire
implementation, counting requested allocations and std::io Read/Write calls.
Its deterministic full-transfer sink is not a measurement of kernel syscalls,
TCP packets or live-service elapsed time. No timing claim uses this diagnostic.

The supplementary read ramp uses the same resource-envelope runner with --read-ramp.
It offers1000/2000/4000/8000 read requests/s for10s per stage, stops on refusal or
p99 strictly greater than twice the preceding stage, and retains the failed stage.
It reports producer, client-queue and call latency separately. A generation span
longer than10/0.95 seconds marks inability to maintain the scheduled rate; even
without that flag, low maximum throughput is not automatically attributed to DB
CPU without evidence. One exploratory ramp per variant is not a capacity proof.

Production defaults remain unchanged unless separately justified by measurements.
