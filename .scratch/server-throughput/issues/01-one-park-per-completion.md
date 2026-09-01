# The server parks once per completion, so concurrency buys nothing

Status: needs-triage

## What was measured

`bench/server/serve.mjs`, a ten-line HTTP server, compiled with `--node --profile
native-release` and driven by `bench/server/drive.mjs` for 5s. Node serving the same program on
the same machine is the control.

| concurrency | kiln rps | kiln p50 | node rps | node p50 |
| --- | --- | --- | --- | --- |
| 1 | 9,106 | 0.105 ms | 32,237 | 0.027 ms |
| 4 | 8,103 | 0.446 ms | 83,927 | 0.044 ms |
| 16 | 9,131 | 1.740 ms | 87,862 | 0.157 ms |
| 64 | 8,196 | 7.806 ms | 85,902 | 0.686 ms |

kiln's throughput is flat from 1 to 64 connections while latency scales linearly with
concurrency: 0.105, 0.446, 1.740, 7.806 ms is almost exactly 64 x 0.122 ms at the top. That is a
server with a fixed 122 us service time and a queue. Node's rises 32k -> 84k over the same range
and only then flattens, which is what overlapping work looks like.

## Where the time goes

`sample` over 6s of load at concurrency 16: 4,359 of 5,163 samples are in `kevent`, reached
through `Runtime::block_on` -> `park_yield` -> `park_internal` -> `Driver::park_thread_timeout`.
Only 804 samples, 0.93s, are CPU. At ~9,000 rps that is about **17 us of CPU per request against
a 122 us service time** - the process is idle for 86% of the time it holds a request.

For scale, node's service time at saturation is 11.6 us. kiln's CPU per request is already in
that neighbourhood. The gap is not the work; it is the waiting.

## The mechanism

Counters in the native drive (parks, and events published per park) held at exactly the same
value at every concurrency:

    concurrency 1    parks=80,000   events=39,997   0.50 events/park   max_ops_in_flight=2
    concurrency 64   parks=80,000   events=39,999   0.50 events/park   max_ops_in_flight=128

Two parks per request, one completion harvested per park, whether 1 request is outstanding or
64. The ops themselves are concurrent - 128 host ops in flight across 66 connections - so the
submission side is fine. It is the harvest that is serial.

## Two hypotheses, both tested and refuted

1. *Only one HTTP read is ever in flight.* No: `max_ops_in_flight` reaches 128.
2. *One reactor turn wakes every ready socket, but `block_on` returns as soon as the first task
   finishes, leaving the rest woken but unpolled.* This one is worth writing down because it
   predicts the numbers exactly. It was implemented - after the awaited `join_next`, yield and
   `collect_ready_nonblocking` in a loop until a round harvests nothing, inside the park already
   paid for - and it harvested nothing. Events per park stayed at 0.50 and rps did not move. The
   sibling tasks are not woken-and-unpolled; at the moment one completion arrives the others are
   genuinely not ready. The change was reverted rather than landed, since an unproven change that
   does nothing is worse than none.

## What to establish next

Why the sibling read completions are not ready at the instant we park, given their sockets have
data and their ops are registered. Suggested first measurement: instrument `TokioHost` to record,
per park, how many tasks are in `self.tasks`, how many were woken by that reactor turn, and how
many completed - the gap between woken and completed is the answer. If the reactor is delivering
readiness for one fd per turn, the question moves to how the transport registers interest.

The prize is worth the instrumentation: 17 us of CPU per request is a ~58,000 rps server if the
idle time goes away, against 9,000 today.

## Context

Follows ADR 0037, whose steps 2 and 3 landed in `66c9e729` and `7e88131d` and took this bench
from 4,692 to 9,056 rps by removing the checkpoint's parks. This issue is the next cost those
parks were hiding, and it is a different one: not the cost of a park, but the number of them.
