# 01. Choose the Node service corpus

Status: resolved

About twenty applications, chosen for what people deploy on Node rather than for what kiln already
runs. Each is a small but realistic app written against the package's documented API, or an
upstream example used unmodified, pinned by lockfile. Proposed set, to be cut or changed:

**Tier 1: M1 and M2 (must pass)**

| app | why it is here |
| --- | --- |
| Express 5 REST API (routing, JSON, middleware, errors) | the most deployed Node framework; `conformance/server/s05` already runs interpreted |
| Fastify 5 API with JSON-schema validation | ajv code generation at startup, plugins (`avvio`); `s04` runs interpreted |
| Hono on `@hono/node-server` | the modern small-framework choice; it has been probed locally (untracked `.scratch/test_hono*.ts`) |
| Koa with a router | a different middleware model (async onion) |
| Express + `pg` (node-postgres) | the most common data layer; plain JavaScript driver, needs `net`, TLS, SCRAM |
| Fastify + `ioredis` (cache and rate limit) | Redis is in most stacks; pipelining, reconnect logic |
| Drizzle ORM + `pg` | an ORM that is plain JavaScript |
| JWT auth: `jsonwebtoken` (HS256 and RS256) + `bcryptjs` | sign/verify and CPU-heavy hashing |
| `zod`-validated API with `pino` logging | the validation and logging most services carry |
| WebSocket server with `ws` | upgrade handling, framing, long-lived connections |
| Outbound HTTPS client (`fetch` + `undici`, calling a local TLS server) | every service calls another service |

**Tier 2: M3**

| app | why |
| --- | --- |
| `mysql2` app | the second most common SQL driver |
| `mongodb` driver app | the document-store case |
| BullMQ worker (on `ioredis`) | background jobs, a non-HTTP long-running process |
| GraphQL server (`graphql-yoga` or `@apollo/server`) | schema building, heavy use of classes and closures |
| tRPC server | TypeScript-first API stacks |
| CLI: `commander` + `chalk` + `fs`, one-shot | the startup-time story; a CLI starts thousands of times |
| React server rendering (`react-dom/server`) behind Express | `conformance/thirdparty/react-ssr` covers the library, not a served app |

**Stretch: M4** (issue 15): Next.js `output: "standalone"`, React Router / Remix server build,
NestJS (decorators, `reflect-metadata`), OpenTelemetry auto-instrumentation, Prisma.

Native addons (`bcrypt`, `better-sqlite3`, `sharp`) are deliberately **not** in the corpus as
passes.
One of them is included as an expected refusal to test issue 06's message.

## Acceptance

- A final list in this file, with each app's package versions and the one-line reason it is in.
- Each tier 1 app has a named owner or is marked unowned.

## Resolution, 2026-09-26

Accepted as proposed by the project owner: the eleven tier 1 apps, the seven tier 2 apps, and the
stretch set. Package versions are pinned when each app is added to `conformance/apps` (issue 02).
Because native addons now have a route (ADR 0067 §4, issue 17), the expected-refusal addon app
becomes a real corpus app once issue 17 lands.
