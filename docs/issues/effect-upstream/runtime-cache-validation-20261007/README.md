# Native runtime cache candidate evidence

This candidate adds a 64-entry realm-owned callable-store lookup cache, reuses
the keyed property-site cache for eligible computed-string reads, and proves
which dead generator locals were already cleared before avoiding repeated clears.
Canonical ownership, conservative fallback, key-domain identity and cache
retirement remain authoritative. No built-in surface or provider ABI changes.

The measured source is CEA landing plus the exact overlays in
`source-provenance.json`; the frozen CLI SHA is in `cli-build-status.json`.
The computed-string fixture was expanded after the CLI build; compiler inputs
remained unchanged. Provider ABI 82 included the callable-store changes.

Focused results on that source:

- Graph fallback checks: 2 passed, including bounded-analysis fallback and a
  missing-entry fallback.
- Backend suspension checks: 10 passed.
- Ordinary affected native probes: 14 matched, 1 refused, 0 diverged. The
  callable-reflection arity-authority refusal remains a support debt, not a pass.
- Affected GC-stress probes: 6 matched, 0 refused or diverged.
- Concurrent-realm probes: 4 matched at 1, 2 and 16 realms, no skips/divergences.

The fresh native ClusterWorkflowEngine replay completed all 47 exact cases:
42 passed and 5 hit the unchanged 5-second timeout; none skipped. Every
case/mode mapped to the official Node/Vitest inventory. This candidate still
needs performance work; it is not a stable improvement claim over older runs.
The stage and original-case replay receipts and raw log are included.

These checks do not prove the whole Effect or native corpus. The fresh quiet native Sharding replay completed all 66 original cases:
62 passed, four reached the original timeout, and none skipped. Its evidence
is in `sharding-replay/`. Both complete 322-file Effect suites (11,019 official
cases), the routine gate, and Bun comparisons remain required. Original 5-second case and 10-second hook deadlines remain.
The earlier landing gate failed with disk-full/linker errors, provider capability
configuration errors, and one worker-output ordering mismatch; it is not a green
gate for this candidate. This commit is a review candidate pending that work.

A fresh diagnostic profile and exact executable-byte-verified symbol attribution
are retained in `native-profile/`. That diagnostic recorded 46/47, with the
original parent interrupt-retry-budget timeout; it is not acceptance evidence.
