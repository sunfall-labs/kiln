# 02: `fs-promises/concurrent-read-order` asserts the same race

Status: done

## Symptom

```
FAIL  node-v24.7.0/fs-promises/concurrent-read-order
  runtime: could not execute module conformance/node/upstream-adapted/fs-promises-concurrent-read-order.js:
  uncaught Error: checkpoint order mismatch
```

6 of 30 idle corpus runs; 30 of 30 under a 16-way CPU load. Same cause as issue 01 — the file is
the same program with `readFile` in place of `writeFile`, so it was always going to travel with
it. It fails less often idle only because reading a 12-byte and a 0-byte fixture is quicker than
creating two files.

## Cause

`setTimeout(0)` as the oracle for two in-flight threadpool reads. Measured on `node v22.23.2`,
100 runs: 30 saw both reads, 63 saw neither, 7 saw one.

## Fix

As issue 01: anchor the assertions on `Promise.all` of the two reads, keep the microtask-before-IO
and both-completed assertions, and keep a timer in the program where its ordering is defined.

## Acceptance

`kiln test conformance/node --node` green over 50 consecutive runs idle and 30 under 16-way load.
