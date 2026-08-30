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
