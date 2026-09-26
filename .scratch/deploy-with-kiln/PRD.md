# PRD: Deploy with kiln

Status: needs-triage
Opened: 2026-09-26
Related: `.scratch/architecture-consolidation/` (how fixes are written),
`.scratch/full-runtime/` (the surface backlog this plan puts in demand order),
`.scratch/native-real-libraries/`, `conformance/server/`, `bench/server/`

## Positioning

kiln competes with Node and Bun. It will not win by asking people to replace their package
manager, test runner and dev habits on day one; Bun spent years making that swap painless, and
it is Bun's strength, not ours. kiln wins where it is structurally ahead of both:

- **Memory.** 8–10 MB resident against Node's ~84 MB and Bun's ~76 MB on the same server
  (`README.md`, 2026-09-02). That is instances per box, and it is on the bill.
- **Startup and artifact.** 8 ms to start, a 4 MB binary with no engine inside it.
- **No warm-up.** The first request after a deploy or a scale-up runs the same code as the
  millionth.

So the promise is about **deployment**, not development:

> Keep Node or Bun for install, dev and tests. Point `kiln build` at your server and ship a small
> native binary that uses a fraction of the memory and behaves identically to Node, or refuses
> at build time and names exactly why.

Replacing `node` for development is a later phase and out of scope here.

## Problem

That promise is only as good as the number of real applications it holds for, and today that
number has not been measured. What exists:

- `conformance/thirdparty`: 22 bundled libraries, almost all utilities, pre-bundled with esbuild.
  They run in both engines and match Node's output byte for byte.
- `conformance/server`: Express (`s04`) and Fastify (`s05`) run **interpreted** in Node mode;
  their `README.md` calls running them native "the next native gates".
- `bench/server/compare.sh`: throughput and memory against Node and Bun, for hello-world and one
  hand-written small service.

What a production Node service needs and kiln does not have, measured 2026-09-26:

- **TLS.** There is no `node:tls`, and `node:https` throws "requires a TLS host capability". Any
  outbound HTTPS call, and any managed Postgres, MySQL or Redis that requires TLS, stops here.
- **Crypto.** `node:crypto` exports hashes, HMAC, random bytes, `randomUUID` and
  `timingSafeEqual`. There is no sign or verify (RS256/ES256 JWTs), no `pbkdf2` or `scrypt`
  (SCRAM auth for Postgres, password hashing), no ciphers, and no `crypto.subtle`.
- **Unbundled projects.** The thirdparty corpus is pre-bundled. A user's project is a
  `package.json` and a `node_modules` tree; whether `kiln build` handles that shape natively, at
  scale, has not been measured.
- **A published answer.** There is nowhere a user can look to see whether their stack works.

## Goal

A **Node service corpus**: about twenty real applications of the kinds people actually deploy,
built **unmodified** from their `package.json` and lockfile, that run natively with responses
identical to Node's, next to a published table of memory, startup and throughput against Node
and Bun *for those apps*.

## Out of scope

- CI changes. Everything here runs locally through scripts and the gate.
- Replacing `npm`/`bun install`, a test runner, a dev server, or `node` for development.
- Interpreter throughput. The interpreter's role here is the fast edit loop and the second
  opinion in differential runs.

## How fixes are written

Every missing member or module an app hits is fixed under the architecture-consolidation rules:
TypeScript in `kiln-builtins` over declared primitives (ADR 0065), and a shared Rust engine behind
the provider where an algorithm needs Rust (ADR 0060 kinds 1 and 2), for example TLS and crypto.
**No new C built-ins and no new duplicates.** A fix that adds a second implementation of a member
increases the duplicate count and the gate refuses it.

## Milestones

| | reads as | apps |
| --- | --- | --- |
| M1 | "an HTTP framework app, native, at 1/8 the memory" | Express, Fastify, Hono, with published numbers |
| M2 | "a typical REST API with Postgres and Redis" | + TLS, crypto, `pg`, `ioredis`, an ORM, JWT auth |
| M2+ | "native addons load" | the N-API subset `bcrypt`, `better-sqlite3` and `sharp` use (issue 17) |
| M3 | "most Node services" | 15 of the ~20 corpus apps |
| M4 | "even the hard ones" | the stretch set: SSR framework builds, NestJS, OpenTelemetry |

## Success measures

| measure | 2026-09-26 | target |
| --- | --- | --- |
| corpus apps with native output identical to Node | 0 of ~20 (none selected yet) | M3: 15 |
| median resident memory, kiln ÷ Node, across passing apps | not measured | publish it; no target until measured |
| startup to first served response | 8 ms hello-world | publish per app |
| throughput vs Node and Bun, per core and whole machine | hello-world and one small service | publish per app |
| distinct refusals the corpus hits | not measured | down, week by week |

## Order

1. **Choose the corpus and build the harness** (01, 02). Everything else is measured against it.
2. **M1** (03, 04, 05): the three frameworks native, unbundled projects building, refusals that
   name the package.
3. **Decisions that shape M2 onward** (06, 10, 13): settled 2026-09-26. See "Decisions" below.
4. **M2** (07, 08, 09): TLS, crypto, the data layer.
5. **Publish** (11, 12), as soon as M1 passes rather than at the end. The table is the product.
6. **Keep going** (13, 14, 15): the deployment story, the weekly burn-down, the stretch apps.

## Decisions

Settled by the project owner on 2026-09-26:

| question | decision | where |
| --- | --- | --- |
| the corpus | the proposed tiers, as written | issue 01 |
| dynamic code, computed `require`, optional deps, addons, patching, run-time files | as ADR 0067 | issue 06 |
| multicore default | one realm; `kiln build --workers=<n\|auto>`; per-core and whole-machine numbers side by side | issue 10 |
| C toolchain | host compiler preferred; a pinned `zig cc` downloaded when there is none or it cannot build the target | issue 13 |
| first deploy artifact | static musl binaries, Linux x86_64 and arm64 | issue 13 |

## Issues

| # | title | status |
| --- | --- | --- |
| 01 | [Choose the Node service corpus](issues/01-choose-the-corpus.md) | resolved |
| 02 | [The app harness](issues/02-app-harness.md) | ready-for-agent |
| 03 | [Express, Fastify and Hono run natively](issues/03-frameworks-native.md) | ready-for-agent |
| 04 | [`kiln build` on an unbundled project](issues/04-build-unbundled-projects.md) | ready-for-agent |
| 05 | [Refusals name the package and the import chain](issues/05-refusals-name-the-package.md) | ready-for-agent |
| 06 | [Closed-world policy: dynamic code, addons, module patching](issues/06-closed-world-policy.md) | resolved (ADR 0067) |
| 07 | [TLS](issues/07-tls.md) | ready-for-agent |
| 08 | [The crypto surface services use](issues/08-crypto-surface.md) | ready-for-agent |
| 09 | [The data layer](issues/09-data-layer.md) | ready-for-agent |
| 10 | [The multicore default](issues/10-multicore-default.md) | ready-for-agent |
| 11 | [Per-app benchmarks against Node and Bun](issues/11-per-app-benchmarks.md) | ready-for-agent |
| 12 | [The compatibility page](issues/12-compatibility-page.md) | ready-for-agent |
| 13 | [The deployment story](issues/13-deployment-story.md) | ready-for-agent |
| 14 | [The weekly burn-down](issues/14-weekly-burn-down.md) | ready-for-agent |
| 15 | [The stretch apps](issues/15-stretch-apps.md) | needs-triage |
| 16 | [The closed-world build behaviours of ADR 0067](issues/16-closed-world-build-behaviours.md) | ready-for-agent |
| 17 | [N-API: the subset real addons use](issues/17-n-api-subset.md) | ready-for-agent |
| 18 | [A build-time instrumentation hook](issues/18-build-time-instrumentation-hook.md) | needs-triage |
