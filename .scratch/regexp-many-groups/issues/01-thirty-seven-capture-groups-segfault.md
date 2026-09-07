# Issue 01: a regular expression with 37 or more capture groups segfaults the native backend

Status: needs-triage
Feature: regexp-many-groups
Governing ADR: ADR 0025

Found on 2026-09-07 while auditing `boxed.c`'s match-array loop for the array-insert GC bug
(`.scratch/array-splice-gc/issues/01`). Unrelated to that bug and to its fix.

## Repro

```js
const n = 37;
const re = new RegExp("(ab)".repeat(n));
const m = re.exec("ab".repeat(n));
console.log(m.length);
```

Written as a literal so the native backend compiles the pattern:

```
$ kiln build /tmp/g37.js --profile native-release --output /tmp/g37 && /tmp/g37
$ echo $?
139
```

`139` is `SIGSEGV`. The interpreter and node both print `38`.

| capture groups | native |
| --- | --- |
| 16, 30, 31, 32, 33, 34, 36 | correct |
| 37, 38, 39, 40 | SIGSEGV (139) |
| 48 | SIGBUS (138) |

It does **not** need `KILN_GC_STRESS`; a plain run crashes.

## Where to look

`kiln_c99_val_read_property`'s regexp-exec path in `crates/kiln-backend-c/src/runtime/boxed.c`
caches match state on the regexp object, and the cache is a fixed 32 groups:

```c
if (g < 32) {
    regexp->cached_elements[g] = bval;
    regexp->cached_ranges[g * 2] = from;
    regexp->cached_ranges[g * 2 + 1] = to;
}
```

The write side is bounded, but the reuse check just above it is not:

```c
int can_reuse = (regexp->cached_group_count == groups);
if (can_reuse) {
    for (size_t g = 0; g < groups; ++g) {
        if (regexp->cached_ranges[g * 2] != ranges[g * 2] || ...
```

That reads `cached_ranges` up to `groups`, past the end whenever `groups > 32`. Whether that is
the crashing read or merely a second bug in the same function needs a debugger; the threshold
sitting at 37 rather than 33 suggests the immediate fault is a fixed-size buffer somewhere else
in the same path (the `ranges` buffer handed to `kiln_regexp_search`, or the match array's
inline storage), so start by finding which allocation is sized for the group count and which is
not.

## Acceptance

- The repro prints `38` on the native backend and matches node at 37, 48 and a few hundred groups.
- A native probe under `conformance/native/` covering a group count above whatever fixed bound is
  found, with the bound named in a comment so the next reader knows why that number.
