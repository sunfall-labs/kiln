# 03. Express, Fastify and Hono run natively

Status: ready-for-agent
Blocked by: 02

M1. `conformance/server/s04-fastify.mjs` and `s05-express.mjs` already run interpreted, and their
README calls native "the next native gates". Take them, plus a Hono app on `@hono/node-server`,
through the harness as corpus apps, and make them match Node natively.

Work the refusal and divergence list in the order the harness reports it. Record each fix's cause
in this file's comments. The `conformance/thirdparty/README.md` habit of recording what each
library found is worth keeping here.

Fastify compiles JSON-schema validators with `new Function` at startup (through ajv). How that runs
natively depends on ADR 0051 (the in-process evaluator) and on issue 06; find out which path it
takes before working around it.

## Acceptance

- All three apps: native output identical to Node, in the harness.
- `s04` and `s05` move into `conformance/apps` or are deleted, not kept as a second copy.
