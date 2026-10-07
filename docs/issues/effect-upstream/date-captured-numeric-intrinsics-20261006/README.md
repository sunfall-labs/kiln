# Date captured numeric intrinsics

The Date bootstrap at `b08688e7` read `Math.floor`, `Math.trunc`, `Math.abs`, `Number.isFinite`, and `Number.isNaN` dynamically. Its setters also called the replaceable global `Number` constructor, which incorrectly accepts BigInt where Date's abstract ToNumber operation must throw.

Candidate commits: `49b6b862` captures the math predicates and helpers at installation; `2e9bac66` changes setter conversions to unary plus, preserving per-call conversion order while rejecting BigInt. Both are based on Date candidate `b08688e7` in the owned worktree `fix/date-intrinsic-capture`.

The checked-in native fixture is `conformance/native/date-captured-numeric-intrinsics.js`. It checks scalar getters and setters after hostile replacement of Math/Number, setter argument order including an invalid Date, invalid-date `Number.isNaN` paths, BigInt rejection for valid and invalid Dates, receiver branding before argument coercion, and the ordinary brand-check error. Node and Bun both print:

```
true true hour,minute,second,millisecond,invalid-date true true true true true true
```

The source-import controls load the exact old `date.ts` from `b08688e7` and the candidate source in separate Node/Bun processes before running the same fixture. The old source throws at `Number.isFinite` after poisoning globals; the candidate passes. A separate setter control shows old `setTime(1n)` returns `1` and old invalid-date `setUTCDate(1n)` returns `NaN`, while the candidate throws `TypeError` for both. Raw logs and runnable adapter scripts are in `evidence/`.

This is host/source-import validation only. No kiln CLI/native corpus run has been performed yet; it awaits the parent-owned Date CLI build lane. The source commits are committed locally and have not been integrated or pushed.

The parent-owned frozen scalar CLI built from clean `b08688e7` was also run in interpreter mode
against the fixture. It exited 1 with `uncaught TypeError: function.isFinite is not a function`,
confirming the failure in kiln's installed Date bootstrap. The exact command, output, and binary
SHA-256 are in `evidence/old-kiln-cli-receipt.txt`, `evidence/old-kiln-interpreter.log`, and
`evidence/old-kiln-cli.sha256`. This is not a native-backend result.

## Provenance

- Old source: `git show b08688e7:crates/semantics/kiln-builtins/src/date.ts`.
- Candidate source: commit `2e9bac66` (with capture change `49b6b862` immediately below it).
- Fixture SHA-256: recorded in `evidence/SHA256SUMS`.
- Node: v26.8.1 with `--experimental-strip-types` for importing the TypeScript bootstrap.
- Bun: v1.4.2.

## UTC time-of-day setter fast path

Commit `95c17bf9` changes only `setUTCHours`, `setUTCMinutes`, `setUTCSeconds`, and
`setUTCMilliseconds`: they now compute the UTC day start and normalized clock directly instead of
converting through civil year/month/day and back. The helper uses the existing captured
`Math.floor`/`Math.trunc`, `Number.isFinite`, and `timeClip` authorities. The setters still read the
receiver's time before coercing arguments, convert all present arguments in order (including
explicit `undefined`), and only then branch on invalid time; BigInt rejection remains unary-plus
ToNumber behavior.

The host differential harness `evidence/utc-setter-controls.mjs` compares the candidate against the
original host setters after importing `date.ts`. It covers eleven boundaries including both
TimeClip limits, explicit undefined, fractions, overflow, infinities/NaN, 1,024 deterministic
randomized cases, argument coercion that mutates the receiver, and invalid-Date coercion side effects.
Prior source is exactly commit `39c7924e`; candidate is `95c17bf9`. Node 26.8.1 and Bun 1.4.2 each
reported 1,384 cases and zero mismatches for both prior and candidate source (raw outputs in
`evidence/utc-*-*.log`). These are host equivalence controls, not a performance claim; no CLI/native
build was run while the parent Date build owned the compile slot.
