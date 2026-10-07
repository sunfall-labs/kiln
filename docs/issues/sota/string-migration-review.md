# String migration review

Status: in progress; not accepted or landed.

The integration branch starts at `76fd5f6`. Implementation uses isolated Luna worktrees. The shared main checkout contains unrelated work and has not been changed.

## Required architecture

- One immutable Latin1/UTF-16 string owner and borrowed view, with direct code-unit indexing.
- One property-key identity and lookup implementation; no scalar shadow key or parallel unit-key API.
- Native strings use the same 16-byte header in constants, GC storage, and boundary values. Header and owned payload share an allocation.
- Internal host values retain exact units. Encoding and replacement happen only at APIs that actually consume bytes.
- One JSON grammar, with narrow and wide storage specializations; no legacy lossy mode or WTF-8 round trip.
- Reuse `regress` Unicode-iterator compilation and UTF-16 matching APIs.
- Preserve allocation-free static strings and avoid per-number allocations in JSON.

## Open acceptance checks

- Interpreter property keys, host ABI, symbol descriptions, inferred names, regex, JSON, and dynamic source preserve lone surrogates.
- Native consumers consistently treat a string boundary pointer as a header, not payload bytes.
- String equality, comparison, concat, slicing, iteration, and indexed reads handle mixed widths and embedded NULs.
- Source literals, templates (including raw templates), and dynamic source retain their distinct semantics.
- ABI/cache versions reject stale artifacts after representation changes.
- Existing parser tests remain intact; new tests cover lossless behavior rather than replacing old coverage.
- Node-oracle tests report code-unit sequences so console encoding cannot hide differences.
- Quiet paired measurements show the expected indexing improvement without unacceptable JSON or HTTP regressions.
- Full gate runs once after the integration is frozen; native GC stress follows allocation/rooting changes.

## Review findings already sent for correction

- Byte-to-unit field renames left half-width `memcmp`/copies and invalid indexing.
- ASCII-only adapters silently replaced non-ASCII dynamic code with an empty body.
- Static string helpers acquired allocations during the migration.
- A surrogate-safe per-character lowercase loop lost Greek final-sigma context.
- Trim returned unmodified strings when any lone surrogate was present.
- Scalar host-value conversion and object tag formatting discarded units inside language operations.
- Duplicated scalar/unit property storage recreated parallel semantic paths.
- Initial JSON migration allocated per numeric token and removed existing test coverage.
- First lossless JSON measurements regressed by roughly 2–5× and were rejected pending diagnosis.

The parser was subsequently consolidated back to one generic grammar after rejecting a duplicated
Latin1 implementation. A sampled numeric profile then identified a lost inline decision: the new
build called `number_from_parts` for each numeric token while the baseline inlined it. A targeted
inline annotation reduced the focused numeric gap from about 13.5% to about 3%; final matched
measurement subsequently showed a roughly 3% numeric improvement. External UTF-8 decoding and parsing an existing JavaScript string are
reported separately.

## Accepted independent pieces

- Owned buffer construction removes a zero-fill-and-copy in the interpreter HTTP buffer path (`6709693a`). No native HTTP throughput gain is claimed.
- Profiled array parameters specialize functions that read `.length` (`1749b53`). The quiet repeated-length fixture improved; indexed access was neutral, so indexed-only functions do not qualify. Full integration validation remains pending.

The corrected accessor fixture exposes a native limitation in non-plain array index descriptors:
`Object.defineProperty` refuses an array index accessor. This is not a general absence of
`Object.defineProperty`. The report records interpreter/oracle agreement and the precise native
refusal separately; it does not claim native accessor coverage.

## Final parser review checkpoint (2026-09-14)

Matched 700 ms rows, 1,000 parses per clock check, three alternating baseline/candidate pairs.
Raw logs: `docs/issues/sota/json-final-matched/`. An earlier run incorrectly passed `--ignored`
to the baseline and ran zero tests; it is excluded in full.

| Input | Baseline median us/parse | Candidate median us/parse |
| --- | ---: | ---: |
| Short ASCII | 0.0165 | 0.0134 |
| Long ASCII | 1.3126 | 1.4443 |
| Escaped | 0.0218 | 0.0213 |
| External UTF-8 Unicode | 0.0196 | 0.0835 |
| Number-heavy | 1.3156 | 1.2755 |

Candidate existing-unit Unicode parse: 0.0169 us/parse. This does not erase external decoding cost.
All five matched checksums agree. Long ASCII and external Unicode regressions remain assigned
for focused profiling and ecosystem bulk codec conversion before acceptance.

Additional open correctness findings: native proxy get still constructed a string value from raw
key bytes; bytecode version remained unchanged after constant representation changed; interpreter
`string_constant` still converted static property keys lossily; frontend auto-accessor backing names
use lossy key formatting, which can collide for distinct lone-surrogate names. Use unique structural
private slot identities or lossless key encoding for generated storage names, and keep inferred
function/class names lossless.

## Fused codec and canonical-view checkpoint

Candidate41af5c52 removes duplicate borrowed views and validates/decodes external UTF-8 with
encoding_rs in one pass. Final binary SHA da1ebe6b0a717dda9b08d432fc9c6c4418cb798eb86df93579edd0d15d92796a.
Three matched alternating pairs, full raw logs and binary hashes in `docs/issues/sota/json-fused-final/`.

| Input | Baseline median us/parse | Final candidate median us/parse |
| --- | ---: | ---: |
| Short ASCII | 0.0166 | 0.0172 |
| Long ASCII | 1.4030 | 1.4277 |
| Escaped | 0.0223 | 0.0273 |
| External UTF-8 Unicode | 0.0197 | 0.0433 |
| Number-heavy | 1.3018 | 1.2671 |

Existing-unit Unicode candidate: 0.0224 us/parse. Checksums agree. External conversion overhead
is substantially lower than the original candidate, but remains above the byte-oriented baseline.
The final canonical-view build also costs more on tiny escaped rows than the earlier candidate;
no overall JSON speedup is claimed. Architecture/correctness cleanup is ready for integration review;
end-to-end runtime measurement remains required to judge the representation tradeoff.

Buffer ownership focused integration test passed (1 passed, pointer reuse and mutation verified),
log `docs/issues/sota/buffer-integration-check.log`.

## Native/interpreter checkpoint

Native c383da45 compiles the backend Rust tests and CLI; generated C also compiles, but actual
execution still throws during builtin bootstrap. Latest object dump shows an Error with string
message/stack fields; decoding the cause and fixing it is assigned to a fresh Luna diagnosis
agent. Stable native589e1f0a fixes Latin1 slice decoding and mixed-width search routing, but
review found its slice(start > end) handling changed empty output into INVALID_ARGUMENT.
That regression is assigned too. Native key interner still uses byte identity/payload-minus-header
and remains a required architectural migration.

Interpreter property-family compile check now succeeds after static-key migration. Tests and
dead-wrapper cleanup remain with its owner. A duplicated linear builtin-name table was rejected
and replaced with borrow-first scalar adaptation at intrinsic-table seams, with wide ASCII
required to behave identically to narrow storage.

Frontend inferred-name slice161eb53d has focused tests; actual auto-accessor execution is still
required because a manually desugared two-private-field fixture does not exercise that lowering.
Symbol descriptions/registry keys remain assigned to the same frontend/name agent.

## Integration review checkpoint — 2026-09-14 afternoon

The preceding progress notes are historical, not the final validation state.

- Native bootstrap was fixed in `4097dc0e`: the old interner had received the new header as bytes. Full native ancestry is integrated through `6f9cd5f6`. The temporary Latin1 bridge is now being replaced by canonical headed keys in the native worktree.
- `ae2e91f3` fixes remaining lossy property definitions (accessors/methods), grouping and combinator keys. The actual TypeScript auto-accessor source now asserts `11 22` followed by `101 202`; interpreter tests passed 179. The dynamic-source harness separately asserts the explicit malformed-source refusal and labels its compiler stub honestly.
- `1d254861` migrates interpreter JSON serialization, replacer keys, raw JSON and indentation to units. Its checked builder retains Latin1 until promotion and appends spans in bulk. Tests passed 182, including exact replacer callback keys and parse round trip.
- `a75c6ba5` merges provider symbol units. The registry uses hashbrown Equivalent with RandomState, allocates only on insertion, and shares canonical StringView hashing. Review verified JSON/regexp sections against `41af5c52`; both enabled implementations and disabled-feature stubs retain that header ABI. Provider feature tests passed 43.
- A native-ancestry merge had dropped the accepted array speculation-policy wiring. `394d2864` restores it, and `5b1b835b` fixes an inferred-name test initializer. The workspace check and both focused array/policy tests now pass.

Still required: finish checked string-conversion propagation, finish and merge native canonical-key/provider callers, run actual native/Node fixtures and GC stress, measure the frozen candidate, then run the full gate. No final gate or landing has occurred.

Native review has rejected separate wide-key property walks, stack headers presented as canonical keys, and pointer-type warnings treated as harmless. The accepted direction is one headed semantic walk, thin byte adapters, stable key exposure, and existing pointer caches. Native JSON review also caught Latin1 bytes entering a UTF8 builder and an unescaped trailing high surrogate; fixes and focused fixtures are being prepared. Fixture review corrected a purported dictionary test from 16 to 40 properties (the fast-shape limit is 32), a purported non-final surrogate, and regex tests that initially used only escaped ASCII patterns.

Raw malformed UTF16 dynamic source is explicitly unsupported at the scalar parser boundary. Native JSON's existing UTF8 output builder also needs a checked indentation boundary: valid Unicode indentation is supported after the ten-unit truncation; an ill-formed truncated indent must refuse explicitly. Do not claim complete native parity for those capability limits.
