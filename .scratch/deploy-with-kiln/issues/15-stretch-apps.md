# 15. The stretch apps

Status: needs-triage
Blocked by: 05

Each is an investigation first: run `kiln check` (05) against it and file what it reports, before
committing to making it pass.

| app | what it will test |
| --- | --- |
| Next.js `output: "standalone"` server | a very large bundle, React server rendering, a lot of dynamic `require` |
| React Router / Remix server build | streaming SSR, Web `Request`/`Response` on Node |
| NestJS | legacy decorators, `reflect-metadata`, dependency injection by reflection |
| OpenTelemetry auto-instrumentation | module patching (`require-in-the-middle`), `AsyncLocalStorage`, `diagnostics_channel` |
| Prisma | its query engine (a native library or WebAssembly, depending on version) |

## Acceptance

- A `kiln check` report for each, filed as issues in this directory.
- A recommendation per app: pursue for M4, or mark as a documented refusal with the reason.
