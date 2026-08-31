# The Node-server probes

Four vertical server programs. The first and third now run under explicit interpreted Node mode;
the later gaps stay visible, and native standalone remains a separate gate.

`docs/design.md` §14 puts the Node compatibility graft at step 10 and the async event loop at
step 11. `docs/language-coverage.md` turns everything before that into a ten-step ladder, and
the corpus is partway up it. Nothing in this directory is reachable from where the ladder
currently stands — that is the point. The rest of the corpus measures the next rung; this
directory marks the destination, so that "run a Node server, compiled" is a gauge with a reading
rather than an intention with a date.

The reading is executable behavior plus precise refusal. `s01` is the minimal end-to-end gate and
`s03` proves request-body streaming and Buffer reassembly; each remaining probe contributes the
next missing prerequisite rather than collapsing the whole target to a compatibility percentage.

## The two things a server breaks, and where they are handled

Both are handled inside the probes, so the runner needed no new machinery for them.

**A server does not terminate, and its output goes to a socket.** Every other probe in this
corpus runs its last statement and exits, and the runner compares its stdout to a recorded
string. A server holds the event loop open forever and writes to a client. So each probe here is
**self-driving**: it listens on port 0, issues a fixed sequence of requests to itself through
`node:http`, prints one line per response, and closes the server. That leaves them ordinary
single-file programs with byte-comparable stdout — and it preserves the corpus's rule that a
failure names a defect rather than a region.

Requests are issued strictly in sequence. Two concurrent requests would make the recorded output
depend on which response completed first, which is a property of the scheduler rather than of
the language.

**A response carries a `Date:` header from the host clock**, and ADR 0015 gave the realm a
virtual clock starting at the epoch. No probe prints a response header that the probe did not
itself set, so none of them can be made to pass by reading the host clock. That collision is
real, and it is what prompted ADR 0031, which turns ADR 0015's closing sentence — "no hidden
dependency to remove later if the decision is revisited, only one to add, in one place,
deliberately" — into four realm capabilities with their current constants as the defaults. Read
with that ADR, these four probes are expected to pass in the *deterministic* realm: a server that
works only when it can read the real clock is a server this compiler has not finished.

## The four

| Probe | What it is for |
| --- | --- |
| `s01-http-hello.mjs` | Whether kiln can be a server at all. Ten lines, no dependencies, a constant body. |
| `s02-routing-json.mjs` | The honest floor for a server someone would write: routing, `await`, JSON, a 404. |
| `s03-request-body-streams.mjs` | Request bodies as `data` events reassembled with `Buffer.concat`. |
| `packages/s04-fastify.mjs` | The ecosystem gate. A real framework, imported by bare specifier. |

`manifest.toml`'s `[server]` table records what each one catches, how its expected output was
observed, and what kiln currently says about it. That file is the specification; this one is the
orientation.

`s01` through `s03` need nothing installed. `s04` imports `fastify` by bare specifier and runs
only where `npm ci` has been run in `packages/`, which is why its status is `needs-packages` —
what `needs-bundle` is to `effect/bundle/`, with one difference that matters: there is no build
step and no generated file. Bundling `s04` the way the Effect programs are bundled would erase
exactly the part still unproven. ADR 0027 already gave bare specifiers a resolution policy and
it already works; Node package resolution and CommonJS interop at the scale of a real dependency
tree is what remains, and the probe keeps its imports so that it keeps asking.

## What passing would be worth

`scripts/server-baseline.sh` measures `s01` under node, deno, bun, and Kiln — as a script, and as a
standalone executable where the toolchain offers one — and writes
`bench/results/server-baseline.json`. All three print the byte-identical line this corpus
records, so it is one program measured three ways rather than three programs.

Taken on `darwin`, medians over 10 runs after 2 discarded, against node v25.9.0, deno 2.9.5 and bun 1.4.0:

| target | form | cold start | peak RSS | artifact |
| --- | --- | --- | --- | --- |
| node | script | 127 ms | 58 MiB | — |
| deno | script | 28 ms | 35 MiB | — |
| bun | script | 25 ms | 25 MiB | — |
| deno | standalone | 27 ms | 35 MiB | **64 MiB** |
| bun | standalone | 22 ms | 24 MiB | **61 MiB** |
| kiln | script | measured by the current baseline script through `kiln run --node` | | — |
| kiln | standalone | not measured — native Node continuations are not advertised yet | | |

The artifact column is the argument. Sixty-odd megabytes is a JavaScript engine, and shipping one
is what `deno compile` and `bun build --compile` do; it is the cost kiln's design declines to pay
by compiling ahead of time to C and linking the small Rust runtime of ADR 0018 instead. Whether
that is worth anything is an empirical question, and the numbers above are the answer's other
half.

The competitor numbers were recorded from a checkout where Kiln scored nothing, deliberately.
That historical baseline remains useful; current runs append Kiln's now-supported interpreted
form without pretending it is a standalone compiled artifact. The script never gates.

## Two findings these produced before they ran once

Recorded here because both are work items rather than curiosities, and both were invisible until
something tried to be a server.

**`node:` was rejected by the resolver, not by the backend.** That finding led to the explicit
`--node` product boundary. Node mode now selects the production Node resolver and typed builtin
graph; ordinary mode still rejects `node:` and unknown builtins fail closed in either path.

**The `fastify` gap is CommonJS interop, not package resolution.** A probe importing only
`fastify` answers `imports missing export 'default'`, naming
`node_modules/fastify/fastify.js` — so ADR 0027's `NodePackageResolver` already found the real
file on disk. What is missing is the interop that synthesizes a `default` for a CommonJS module.

## And one that changed the harness

The probes are `.mjs`, and that is a finding rather than a preference.

`kiln-conformance` parsed every program under the Script goal, deliberately: the corpus is
mostly scripts, its expected output was observed from scripts, and the module goal would let
module-only syntax be silently accepted in programs never run that way. The first draft of these
four was `.js`, so all four refused with `Cannot use import statement outside a module` — an
answer that named the runner's default rather than kiln's server gap.

The runner now takes the goal from the extension: `.mjs` is a Module, `.js` stays a Script, and
the guarantee that comment was protecting is intact. What it deliberately does **not** do is
read the nearest `package.json` `type` field, which is how Node decides the same question —
`packages/package.json` says `"type": "module"` and has no effect. Real Node applications are
overwhelmingly `.js`-plus-`type`, so that is itself an item for step 10. These four sidestep it
on purpose, so that they measure the thing they were written to measure.
