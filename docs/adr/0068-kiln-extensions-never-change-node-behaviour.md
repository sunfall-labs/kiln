# ADR 0068: kiln's own built-ins never change Node behaviour

## Status

Accepted, 2026-09-26. Decided by the project owner.

Refines ADR 0063 (Node is the target of both engines) for members that Node does not have. Uses
ADR 0043 and ADR 0059 (a built-in member is described once) as the place the distinction is
recorded.

## Context

kiln adds built-ins Node does not have, starting with `Array.prototype.parallelMap` and the
`parallelMap` of each typed array (ADR 0058). They are meant to be real built-ins that feel native,
installed on the prototypes like everything else, not a library a program imports.

Every such member is observable in principle. A program that never looks for it behaves
identically, but `'parallelMap' in []` answers `true` where Node answers `false`, and
`Object.getOwnPropertyNames(Array.prototype)` lists one more name. The programs that actually
depend on the exact set of built-in members are few, but they exist: hardening libraries such as
SES `lockdown()` compare the built-ins with a list they expect, and may warn about or remove members
they do not recognise.

Until now nothing recorded which members are kiln's own, so the surface inventory counts
`parallelMap` as "extra (node lacks it)" beside members that are genuinely wrong, and a
`@kiln/concurrency` fallback and the engine's own method could both define it.

## Decision

**A kiln extension may be present, but it may never make a program that uses only Node's surface
behave differently from Node.**

1. **Declared.** Every kiln extension is marked as one in the built-in member description
   (ADR 0059), with its owner, like any other member. The surface inventory and the Node-parity
   corpora read that mark and count it as intentional, never as a divergence.
2. **Non-enumerable.** Extensions never appear in `for...in`, spread, `Object.keys`,
   `Object.entries`, `JSON.stringify` or `util.inspect`.
3. **Invisible when used.** Where an extension optimises something Node can express (a parallel
   `map`), its results, exceptions and side effects are exactly those of the Node equivalent. Only
   time and core usage may differ. Where no Node equivalent exists, its semantics are specified
   once and are deterministic: independent of core count, scheduling and engine.
4. **On by default, with one switch to remove them all.** `kiln build --no-kiln-extensions` for a
   binary, and `KILN_EXTENSIONS=0` at run time for either engine, install exactly Node's surface.
   The switch is for the rare program that audits the built-ins; nobody should need it otherwise.
5. **One definition.** The engines own the extension. A package that also offers it defines its
   fallback only when the method is absent, so under kiln there are never two definitions
   competing. (Since 2026-10-02 no kiln package offers one: `@kiln/concurrency` is removed, and
   `@kiln/types` only declares the engine's members.)
6. **Proven, not argued.** Test262, `conformance/thirdparty` and the deploy corpus
   (`.scratch/deploy-with-kiln/`) run with extensions installed and must stay identical to Node. A
   regression an extension causes fails the gate like any other divergence.

### 2026-10-02 amendment: one name per module, Node's modules exactly Node's

**Names.** Every module kiln adds is `kiln:<library>` (and `kiln:<library>/<subpath>`), as Node's
are `node:<name>` (ADR 0074 §1 gives the reasons). No package re-exports one under a second name;
`@kiln/concurrency` (a second name for `kiln:actors`) and `@kiln/effect` were removed for that.
Editors get every `kiln:` module's types, and the parallel collection methods', from one package,
`@kiln/types`, generated from the engine's sources by `scripts/gen-kiln-types.sh`, the way
`@types/node` describes `node:*`.

**A `node:` module is Node's and nothing else.** `node:process` (and bare `process`) is the global
`process`: its default export is that object and its named exports are its own enumerable
members, so `import { on } from "node:process"` fails at link time exactly as in Node. kiln's
process API (`run`) is `kiln:process`, never a member of `process` or a name in Node's namespace.
`process.cwd`, `exit`, `kill`, `nextTick`, `hrtime` and `getBuiltinModule` are enumerable own
properties and the emitter methods inherited, as in Node; host hooks are not enumerable.

**One virtual path per module.** A builtin's virtual path carries its scheme --
`.kiln/builtins/<scheme>/<name>.builtin`, so `.kiln/builtins/node/process.builtin` and
`.kiln/builtins/kiln/process.builtin` -- and the path names exactly one specifier
(`ModuleSpecifier::virtual_path` and its inverse, `builtin`). Before, `node:process` and
`kiln:process` shared a path.

## Amendment 2026-10-02: modules, globals and private channels

Rule 4's switch removes every kiln extension, and an audit found four places it did not:

- **Modules.** With extensions off, every `kiln:` module refuses, internal ones included, with one
  error in both engines and for static and dynamic imports alike: a `TypeError` naming the module
  (`kiln:actors requires enabled Kiln extensions`; `kiln:actors/core` and `kiln:actors/worker` name
  their owner). The refusal is the module's own first statement, so nothing about it is engine
  specific. `kiln:actors/core` and `kiln:actors/worker` are engine-internal: only an engine builtin
  may import them (the resolver and the interpreter's dynamic loader refuse any other importer with
  `` `kiln:actors/core` is internal to kiln:actors ``), and `new Worker("kiln:actors/worker")` from a
  program is a `ERR_WORKER_PATH` like any string that is not a path.
- **Globals.** `Kiln` and `Bun` are declared extensions (`owner: "globalThis"`), installed only
  when extensions are, and non-enumerable.
- **Private channels.** Nothing the engine passes between its own modules is a property a program
  can find. There is one channel, a realm-private object, `__kilnPrivate`. The bootstrap captures it
  from the global object and deletes the name before any program line runs; each builtin module
  can read `globalThis.__kilnPrivate` only while its own top-level code runs, in a window the
  engine opens and closes around that code (native: a read overlay and a per-realm GC-rooted slot;
  interpreter: one global property added and removed again). It carries `extensions`, the
  engine's operations that exist only with extensions (`share`, `sharedIs`, `hostProcessStart`,
  `hostProcessCancel`), and what bootstrap pieces publish for modules (`messagePort`,
  `structuredClone`, `senderEndpoint`, `spawnActorWorker`). No `Symbol.for("kiln...")` key remains
  on any built-in, and a program cannot forge an actor mark, which is an unregistered symbol.
- `KILN_ENGINE_PRIVATE_FOR_TESTS=1` leaves the name readable. It exists for the conformance probes
  that drive the actor protocol directly and is off in both engines unless a developer sets it.

## Consequences

- kiln can grow built-ins that make it more capable than Node without becoming less compatible.
- The surface inventory's "extra" column stops mixing kiln's intended members with mistakes.
- Every extension carries a burden of proof: identical behaviour, measured by the corpora, and a
  switch that removes it.

## Rejected alternatives

**Ship extensions only as an importable package.** Keeps Node's surface exactly, but makes kiln's
own capabilities feel like a library rather than part of the runtime, which is the opposite of the
intent.

**Off by default, opt in.** Zero risk, but users have to know the feature exists before they can
find it, and most never will.

**Always on, no switch.** Simplest, and leaves a program that audits its built-ins with no way to
run on kiln unmodified.
