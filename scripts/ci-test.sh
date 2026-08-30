#!/usr/bin/env bash
#
# Run the workspace test suite.
#
# cargo-nextest is the better runner - process-per-test, so a panicking test cannot take its
# neighbours with it, and much clearer failure output - and it is what developers have
# installed locally. It is not on a stock GitHub runner, and for a workspace this small the
# install costs more wall clock than the runner saves, so CI takes the fallback path on
# purpose. The two branches must cover the same tests: nextest does not execute doctests, so
# that branch runs them separately.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

# --locked everywhere: a manifest change that was not accompanied by a Cargo.lock update is a
# reproducibility bug, and CI is the right place to notice it.
if cargo nextest --version >/dev/null 2>&1; then
	echo "test runner: cargo-nextest"
	cargo nextest run --workspace --locked
	cargo test --workspace --locked --doc
else
	echo "test runner: cargo test (cargo-nextest is not installed)"
	cargo test --workspace --locked
fi

# The number provider is an rlib during ordinary workspace tests and a no-std, panic-abort static
# archive in shipped artifacts. Exercise that distinct packaging/link contract from a C caller.
scripts/check-native-number-ffi.sh

# Exercise the public conformance workflow, not only its component crates. The local corpus is
# intentionally tiny; the full upstream suite remains an external, separately pinned input.
cargo run -p kiln-cli --locked -- --one-shot test tests/conformance/smoke

# Exercise the public native path against the same bytecode as the interpreter. The second build
# must be a fully warm artifact-cache hit, and both execution engines must expose identical stdout
# for the supported scalar slice.
KILN_CI_NATIVE_OUTPUT="target/kiln-ci-native"
cargo run -p kiln-cli --locked -- --one-shot build examples/hello.ts \
	--output "$KILN_CI_NATIVE_OUTPUT" >/dev/null
KILN_CI_WARM_BUILD="$(cargo run -p kiln-cli --locked -- --one-shot build examples/hello.ts \
	--output "$KILN_CI_NATIVE_OUTPUT")"
KILN_CI_HIT_COUNT="$(printf '%s\n' "$KILN_CI_WARM_BUILD" | grep -c '(hit)')"
if [[ $KILN_CI_HIT_COUNT -ne 5 ]]; then
	echo "expected five native cache hits on the warm build" >&2
	printf '%s\n' "$KILN_CI_WARM_BUILD" >&2
	exit 1
fi
KILN_CI_INTERPRETER_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run examples/hello.ts)"
KILN_CI_NATIVE_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run examples/hello.ts --native)"
if [[ $KILN_CI_NATIVE_STDOUT != "$KILN_CI_INTERPRETER_STDOUT" ]]; then
	echo "native stdout differs from interpreter stdout" >&2
	printf 'interpreter: %s\nnative: %s\n' "$KILN_CI_INTERPRETER_STDOUT" "$KILN_CI_NATIVE_STDOUT" >&2
	exit 1
fi

# Exercise the public graph path as well: exact relative resolution, dependency-first side
# effects, an imported scalar value, per-module native artifacts, and interpreter/native
# equivalence all have to survive the CLI/daemon boundary.
KILN_CI_MODULE_OUTPUT="target/kiln-ci-native-modules"
cargo run -p kiln-cli --locked -- --one-shot build examples/modules/main.ts \
	--output "$KILN_CI_MODULE_OUTPUT" >/dev/null
KILN_CI_MODULE_WARM_BUILD="$(cargo run -p kiln-cli --locked -- --one-shot build \
	examples/modules/main.ts --output "$KILN_CI_MODULE_OUTPUT")"
KILN_CI_MODULE_HIT_COUNT="$(printf '%s\n' "$KILN_CI_MODULE_WARM_BUILD" | grep -c '(hit)')"
if [[ $KILN_CI_MODULE_HIT_COUNT -ne 5 ]]; then
	echo "expected five native graph cache hits on the warm build" >&2
	printf '%s\n' "$KILN_CI_MODULE_WARM_BUILD" >&2
	exit 1
fi
KILN_CI_MODULE_INTERPRETER_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run \
	examples/modules/main.ts)"
KILN_CI_MODULE_NATIVE_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run \
	examples/modules/main.ts --native)"
KILN_CI_MODULE_EXPECTED=$'dependency\n42'
if [[ $KILN_CI_MODULE_INTERPRETER_STDOUT != "$KILN_CI_MODULE_EXPECTED" || \
	$KILN_CI_MODULE_NATIVE_STDOUT != "$KILN_CI_MODULE_EXPECTED" ]]; then
	echo "linked module output is wrong or differs between execution engines" >&2
	printf 'expected: %s\ninterpreter: %s\nnative: %s\n' "$KILN_CI_MODULE_EXPECTED" \
		"$KILN_CI_MODULE_INTERPRETER_STDOUT" "$KILN_CI_MODULE_NATIVE_STDOUT" >&2
	exit 1
fi

# A function value that crosses a module boundary. Interpreter only: the C backend is still
# scalars-only, so asking it to link this would be testing a refusal rather than the seam.
# What is being gated is that an exported function, class, arrow and default all run, and that
# each reads its *own* module's bindings rather than the importer's same-numbered slot.
KILN_CI_EXPORTS_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run \
	examples/modules/exports/main.ts)"
KILN_CI_EXPORTS_EXPECTED=$'3cm\nbox 4cm\n42\n0cm in'
if [[ $KILN_CI_EXPORTS_STDOUT != "$KILN_CI_EXPORTS_EXPECTED" ]]; then
	echo "an exported function did not see its own module" >&2
	printf 'expected: %s\nactual: %s\n' "$KILN_CI_EXPORTS_EXPECTED" "$KILN_CI_EXPORTS_STDOUT" >&2
	exit 1
fi

# Every forwarding form: a named re-export, a rename, the default under a new name, a namespace,
# and a star whose names lose to one the forwarding module exports itself.
KILN_CI_REEXPORTS_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run \
	examples/modules/exports/via.ts)"
KILN_CI_REEXPORTS_EXPECTED=$'1cm 4 0cm box 3cm\nmm metric\nBox,default,describe,double\ntrue'
if [[ $KILN_CI_REEXPORTS_STDOUT != "$KILN_CI_REEXPORTS_EXPECTED" ]]; then
	echo "a re-export forwarded the wrong binding" >&2
	printf 'expected: %s\nactual: %s\n' "$KILN_CI_REEXPORTS_EXPECTED" "$KILN_CI_REEXPORTS_STDOUT" >&2
	exit 1
fi

# An import is a live view of the exporter's binding, not a copy of the value it had when the
# exporter finished. Only a binding that can change is boxed, which is why `examples/modules`
# above still compiles natively: its one export is a `const`.
KILN_CI_LIVE_STDOUT="$(cargo run -p kiln-cli --locked -- --one-shot run \
	examples/modules/live/main.ts)"
KILN_CI_LIVE_EXPECTED=$'0 10\n2 10'
if [[ $KILN_CI_LIVE_STDOUT != "$KILN_CI_LIVE_EXPECTED" ]]; then
	echo "an imported binding did not track the exporter" >&2
	printf 'expected: %s\nactual: %s\n' "$KILN_CI_LIVE_EXPECTED" "$KILN_CI_LIVE_STDOUT" >&2
	exit 1
fi
