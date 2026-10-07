# 09. The data layer

Status: ready-for-agent
Blocked by: 02, 07, 08

M2's corpus apps: Express + `pg`, Fastify + `ioredis`, Drizzle + `pg`; then `mysql2` and `mongodb`
for M3. These are plain-JavaScript drivers on `net`, `tls`, `crypto`, `Buffer`, `events` and
`stream`, so they test the Node surface under load rather than any single member.

What to watch for beyond missing members:

- backpressure and partial reads on `net.Socket` (large result sets, pipelined Redis commands);
- reconnect and error paths (kill the service mid-run in one request script);
- connection pools holding the event loop open, and closing cleanly on shutdown;
- `Buffer` performance on the hot path, which the throughput numbers will show.

## Acceptance

- The three M2 data apps match Node natively, including one scripted connection-loss scenario.
- Throughput for each recorded in issue 11's table.
