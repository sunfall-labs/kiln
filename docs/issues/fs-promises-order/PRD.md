# PRD: the two concurrent `fs.promises` ordering probes assert a race

Status: done
Branch: `fix/fs-promises-order` (worktree `.claude/worktrees/agent-a50a0fa74f268a0d2`)
Owner: Claude (session 016duSRiZ6duBoS1bmWq7Yd1), 2026-09-03

## Problem

`kiln test conformance/node --node` fails intermittently, roughly one run in three on an idle
machine and **every run under load**, on two tests:

- `node-v24.7.0/fs-promises/concurrent-write-order` — `concurrent completion order mismatch`
- `node-v24.7.0/fs-promises/concurrent-read-order` — `checkpoint order mismatch`

Both are derived compatibility probes (`conformance/node/UPSTREAM.md` says so — they are not
byte-for-byte upstream Node files), and both have the same shape:

```js
fs.writeFile(first, 'a').then(() => seen.push('a'));
fs.writeFile(second, 'b').then(() => seen.push('b'));
Promise.resolve().then(() => seen.push('microtask'));
setTimeout(() => {
  if (seen[0] !== 'microtask' || seen.length !== 3 || ...) throw new Error(...);
}, 0);
```

The `seen.length !== 3` clause is the assertion that a `setTimeout(0)` callback runs *after* two
filesystem operations that were still in flight when the timer was armed.

## What is actually true

Nothing guarantees that, in Node or in Kiln. A `setTimeout(0)` is a due timer; an `fs.promises`
write is threadpool work. Whether the threadpool wins is a wall-clock race.

Measured on real `node v22.23.2` on this machine, 100 runs of the same program, reporting `seen`
inside the `setTimeout(0)` callback instead of throwing:

```
58  ["microtask"]
35  ["microtask","a","b"]     <- the only shape the conformance test accepts
 7  ["microtask","a"]
```

The read probe, 100 runs: 30 `["microtask","text:12","empty:0"]`, 63 `["microtask"]`,
7 `["microtask","text:12"]`. Under a 16-way CPU load the write probe drops to 8 of 40.

So the assertion the corpus makes holds **30–35% of the time in Node itself**, and 20% under load.

What *is* defined, and what both engines do reliably:

- the microtask checkpoint after the script drains before any filesystem completion is delivered
  (`seen[0] === 'microtask'` in 100 of 100 Node runs and in every Kiln run measured);
- both operations complete with the right values, concurrently;
- a timer armed from inside a completion callback runs on a later turn than that callback.

What is **not** defined, and which the probes must therefore stop asserting: that an already-due
`setTimeout(0)` waits for in-flight filesystem work. Node's own completion order between the two
concurrent writes is not stable either — 1 of 100 runs settled `b` before `a`.

## Non-goals

Retries, sleeps, sorts, and "assert less" are all excluded. The replacement anchors the ordering
assertions on an event where the order *is* defined (both promises settled) rather than on a
timer that races them, so the same behaviour is pinned without a race in the oracle.

## Engine position

The engine is not at fault and is not changed.
`RealmRuntime::next_turn` documents the rule deliberately: "Waiting on I/O first would starve
timeouts behind an open socket; running virtual timers first would make every timeout beat
already-ready I/O", and biased selection gives an *already queued* completion priority over the
timer, while an in-flight one does not delay it. NOTES.md 2026-09-02 ("A due timer is not a wait
case") moved the native drive the same way, for the same reason. Node mode is also the one mode
that deliberately observes ambient I/O (`UPSTREAM.md`: "Ordinary deterministic conformance
continues to omit `--node`, so it never observes ambient I/O"), so real disk latency is in scope
by construction and cannot be scheduled away.

Completion delivery order is also already submission-ordered where it can be:
`TokioHost::take_ready` pops the lowest submission sequence present in the ready map.

## Issues

- `issues/01-write-order-probe-asserts-a-race.md`
- `issues/02-read-order-probe-asserts-a-race.md`

## Result

| | write-order | read-order | corpus runs |
| --- | --- | --- | --- |
| before, idle | 2 | 6 | 30 |
| before, 16-way load | 30 | 30 | 30 |
| before, standalone `kiln run --node --one-shot` | 30/30 | 30/30 | — |
| after, idle | 0 | 0 | 50 |
| after, 16-way load | 0 | 0 | 30 |
| after, standalone `kiln run --node --one-shot` | 0/30 | 0/30 | — |

Both rewritten files also pass real `node v22.23.2` 60/60 idle and 40/40 under a 16-way load; the
timer-anchored originals passed it 35% and 30% of the time respectively.

Verification: `kiln test conformance/node --node` 35/35; `mbx test -p kiln-runtime -p
kiln-interpreter -p kiln-native --offline --no-fail-fast` all green (142 + 88 + 3 + 99 passed, 0
failed); `mbx test -p kiln-cli -p kiln-conformance --offline --no-fail-fast` all green;
`bash scripts/native.sh` 76 matched, 2 refused, 0 diverged.
