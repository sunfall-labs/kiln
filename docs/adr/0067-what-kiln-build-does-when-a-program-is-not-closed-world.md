# ADR 0067: What `kiln build` does when a program is not closed-world

## Status

Accepted, 2026-09-26. Decided by the project owner for `.scratch/deploy-with-kiln/` (issue 06).

Applies ADR 0063 (a refusal is a debt) to the parts of real Node services that a closed-world
native build cannot see at build time. Relies on ADR 0051 (an in-process evaluator for dynamic code
in the native realm) and ADR 0062 (demand-driven linking).

## Context

A native build is closed-world: it compiles the modules it can prove the program reaches, and
nothing else. Real Node services break that assumption in six recurring ways, and each needs its
own answer, because the right answer for generated code is wrong for a native addon:

1. code generated at run time with `new Function` or `eval` (ajv and fast-json-stringify, which
   Fastify uses at startup; template engines);
2. `require` with a computed specifier (plugin loaders, drivers choosing an implementation);
3. optional dependencies that may not be installed (`pg-native`, `bufferutil`, `utf-8-validate`);
4. native addons, `.node` shared libraries built against N-API (`bcrypt`, `better-sqlite3`,
   `sharp`);
5. module patching at load time (OpenTelemetry auto-instrumentation and APM agents, through
   `require-in-the-middle` and `import-in-the-middle`);
6. files a package reads at run time relative to itself (templates, `.wasm`, JSON data, locale
   files).

Without a policy each gets decided ad hoc, at the call site, by whoever hits it first.

## Decision

**`kiln build` makes the program work when it can prove how, uses a declared answer when the user
gives one, and otherwise refuses at build time naming the package, the call site and the way
forward.** A refusal here is a debt under ADR 0063 unless this ADR says it is permanent, and none
of the six is permanent.

### 1. Generated code

When the source passed to `new Function` or `eval` is determined at build time (a constant, or a
value the build can evaluate from constants and the program's own static data, such as a JSON
schema), the build compiles it natively as if it were written in the program. Otherwise the call
runs at run time in ADR 0051's in-process evaluator. The program works either way; precompiling
is how the hot path of a schema validator or serializer runs native.

### 2. Computed `require` and `import()`

When static analysis proves the specifier is one of a finite set (a conditional between literals,
a template over a known directory listing), every member of the set is included. The user may also
declare extra modules to include, in the project's `package.json` or on the command line. Anything
else is refused at build, naming the call site and showing how to declare the modules.

### 3. Optional dependencies

A dependency that is optional and not installed behaves exactly as it does under Node: the
`require` throws `MODULE_NOT_FOUND` at run time, and the package's own fallback runs. The build
does not refuse and does not warn, because Node does neither.

### 4. Native addons

`.node` addons are loaded at run time through an N-API implementation over kiln's values, built on
`node:ffi`. The first implementation is the subset of N-API that `bcrypt`, `better-sqlite3` and
`sharp` use, measured from their symbol tables, and it is scheduled after the deploy plan's M2
(TLS, crypto and the data layer). Until an addon's N-API needs are met, the build refuses it,
naming the package and a pure-JavaScript or built-in alternative where one exists (`bcryptjs`,
`node:sqlite`).

An addon is a platform-specific shared library, so it ships beside the binary rather than inside
it; whether to embed and extract instead is left to the N-API work.

### 5. Module patching

Refused at build now, naming the instrumentation package, and tracked as a debt. The intended
answer is a build-time instrumentation hook: the closed-world equivalent of patching a module as it
loads, so that OpenTelemetry can instrument a program statically. That hook is its own ADR, written
when the work is scheduled.

### 6. Files read at run time

When a file's path is provable at build time (`path.join(__dirname, "schema.json")`,
`new URL("./x.wasm", import.meta.url)`), the file is embedded and the read is served from the
binary. The user may declare further assets to embed. Any other read goes to disk beside the binary,
as it does under Node, and the build prints a warning naming the path, so a one-file deployment
that is missing a file is found at build time rather than in production.

### How a refusal reads

Every refusal under this ADR follows the deploy plan's issue 05: the member or construct, the
package and version, the file and line, the import chain from the entry point, the tracking issue,
and the way forward (declare it, or use the named alternative). `kiln check` lists every one of
them in a single pass.

## Consequences

- Fastify and other ajv users build and run without configuration; the only question is how much
  of their generated code runs native.
- A user's first contact with a closed-world limit is a build-time message with a fix, not a
  production crash.
- N-API becomes a planned milestone rather than a permanent gap, which puts `bcrypt`,
  `better-sqlite3` and `sharp` on the road map instead of in a list of unsupported packages.
- Embedded files make the one-file deploy the default, and the warning makes it honest when it is
  not.

## Rejected alternatives

**Refuse all dynamic code.** Strict and simple, and it excludes Fastify and every ajv user, which is
a large part of the Node server ecosystem.

**Always use the evaluator for dynamic code.** Correct, but leaves validators and serializers
interpreted on the hot path of the services this project is trying to win.

**Include every module under `node_modules` a computed `require` could match.** Always builds,
at the cost of binary size and build time for every user to spare a few from declaring modules.

**Refuse native addons permanently.** Simpler, but gives up packages many real services depend
on, when Bun has shown N-API over a non-V8 engine is achievable.

**Emulate module patching at run time.** Closest to Node, but it requires every module to stay
patchable after the build, which works against the closed-world optimizations that are the point of
building natively.
