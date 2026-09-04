# `kiln-native`'s tests fail in the gate and pass on their own

**Status:** open. Not a correctness bug -- a *measurement* one, which is worse in its own way: the
gate goes red and blames whatever change is being measured.

## What happens

`mise run gate` reported `test result: FAILED. 85 passed; 4 failed` for `kiln-native`, twice on
different changes. Run alone the same suite is `89 passed; 0 failed`, and it stayed clean across:

- `mbx test -p kiln-native --lib` twice,
- `mbx test -p kiln-native --lib -- --test-threads=8` twice,
- `mbx test --workspace -- --test-threads=4` five times.

The only condition that has ever produced it is inside `scripts/gate.sh`, immediately after the
release build the gate does first. It survived lowering `--test-threads` to 4.

## Why it is probably contention

Several of these tests each shell out to a C compiler. `mbx` shares one CPU budget across
concurrent invocations, which is the point of it, but a test binary's own thread pool is outside
that budget -- so the tests can be running compilers while the budget is already spent. The gate is
the only place that reliably arranges for a release build to have just finished.

That is a hypothesis, not a diagnosis. What is certain is the shape: it fails only under load, and
only in the one place that guarantees load.

## What to do about it

Not a retry. A gate that passes on the second attempt is a gate that has stopped meaning anything,
and this is exactly the class -- an intermittent red that trains people to re-run -- that makes a
real regression invisible.

The first step is to know *which* four. `gate.sh` now prints the failing test names as well as the
summary, so the next occurrence names them; before that it printed only counts, which is why this
filing cannot say which tests they were. With names, the question becomes whether they share a
timeout, a temp directory, or a compiler invocation, and that is answerable.
