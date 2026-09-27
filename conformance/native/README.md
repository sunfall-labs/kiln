# Native equivalence corpus

Programs that both engines must run, printing the same bytes.

The interpreter is the oracle here, not node. That is deliberate and it is the only place in
this repository where it is true. `conformance/differential` asks whether kiln agrees with
JavaScript; this asks a narrower question with a sharper answer: **whether kiln's two execution
engines agree with each other.** They compile the same bytecode, so any disagreement is a bug in
one of them and nothing else -- no specification reading, no node version, no locale.

A probe belongs here when it exercises something the native backend has just learned. Keep it to
constructs the backend actually supports: `kiln run --native` refuses anything else by name
(`unsupported native emission: \`NewObject\` in the native backend`), and a refusal is reported
separately from a disagreement, because a refusal is a backlog entry and a disagreement is a bug.

Run it with `scripts/native.sh`. Both engines are driven through the CLI, so this covers the
whole path -- lowering, emission, the C toolchain, the runtime and the HostOps boundary -- rather
than the backend crate in isolation, which its own unit tests already do.

## Probes that need a release build: the `kiln-native:` directive

A probe may name, in its first five lines, a profile and a set of thread counts:

```js
// kiln-native: profile=native-release parallelism=1,2,max
```

`scripts/native.sh` then builds it with `--profile native-release` and runs the binary once per
`KILN_PARALLELISM` value (`max` is the core count); every run must print the interpreter's bytes.
The `parallel-*.js` probes -- the parallel/sequential differential harness of
`.scratch/parallel-builtins` issue 02 -- use it, because only a release build emits the parallel
lowering of `parallelMap`, and because a result that depends on how many threads computed it is a
divergence like any other. A release module past 16 functions or 512 instructions is emitted
generic-only, but its qualifying `parallelMap` sites keep their parallel lowering (issue 13):
most probes stay under the limit so their bodies are specialized, and `parallel-large-module.js`
covers the generic case. The development profile emits no parallel code, and `--explain-parallel`
says `dev-profile` there. `kiln build --profile native-release --explain-parallel <probe>` says
which sites run in parallel. The family (`parallel-filter`, `parallel-flatmap`, `parallel-reduce`,
`parallel-sort`, `parallel-family-refused`; issue 10) is held to `filter`, `flatMap` and `sort`,
and `parallelReduce` to its fixed tree written out again in the probe.
