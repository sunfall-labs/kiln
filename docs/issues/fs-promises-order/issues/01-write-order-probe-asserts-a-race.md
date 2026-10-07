# 01: `fs-promises/concurrent-write-order` asserts a race Node does not win

Status: done

## Symptom

```
FAIL  node-v24.7.0/fs-promises/concurrent-write-order
  runtime: could not execute module conformance/node/upstream-adapted/fs-promises-concurrent-write-order.js:
  uncaught Error: concurrent completion order mismatch
```

Roughly 1 run in 3 idle; 30 of 30 under a 16-way CPU load; 30 of 30 when the module is run on its
own with `kiln run --node --one-shot` (the corpus run warms the Tokio blocking pool, a standalone
run does not, which is exactly the tell that this is wall-clock latency and not ordering).

## Cause

The oracle is `setTimeout(0)`, and it asks whether two in-flight threadpool writes finished first.
Measured on `node v22.23.2`, 100 runs: 35 saw both writes, 58 saw neither, 7 saw one. The
conformance file accepts only the first shape.

## Fix

Anchor on `Promise.all` of the two writes — the point at which the order *is* defined — and keep
every assertion that is real: the microtask checkpoint precedes every filesystem completion, both
writes complete, and a timer armed from inside the completion callback runs later still. Add the
readback that proves the bytes landed, so the case gets stronger rather than weaker.

## Acceptance

`kiln test conformance/node --node` green over 50 consecutive runs idle and 30 under 16-way load.
