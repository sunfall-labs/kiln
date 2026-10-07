# Inherited array index writes

Follow-up to the shared numeric-index guard in `b7c8791e`. The guard correctly sends
writes to the generic path after a child call changes an intrinsic prototype, but that
path had two existing correctness gaps:

- `write_accepted_key` bypassed materialized descriptors on `Native` prototypes and
  inspected only the static member table. Getter-only and nonwritable numeric properties
  installed on `Array.prototype` or `Object.prototype` were consequently ignored.
- `set_array_index` created an own element without consulting inherited setters or
  nonwritable data properties. Its creation checks also treated holes inside a
  nonextensible array differently from missing elements past the end.

The correction deletes the special Native branch and uses `own_property_state_key`, the
same descriptor authority already used for ordinary properties and reflection. Missing
array elements use the existing `chain_setter_key` before testing creation constraints.
Existing own elements still shadow inherited properties. Numeric and string index writes
share this path, including frozen arrays whose inherited setter creates no own element.
One representation helper owns missing-element creation eligibility (extensibility and
array length writability) for both the strict precheck and actual write. The existing
shared monotonic indexed-prototype flag avoids a prototype walk when interception is
impossible; the Unicode optimization's `set_or_throw` guard is unchanged.

No new built-in, semantic engine, prototype representation, budget, classification or
baseline is introduced. The now-unused static-accessor predicate wrapper was deleted;
its existing unit assertion uses the underlying accessor query directly.

## Validation

- The source regression installs a getter-only `Array.prototype[0]` from a child call,
  then tests strict refusal and sloppy non-creation. It also covers exact numeric/string
  keys, negative zero, setter receiver and call count, nonextensible/frozen receivers,
  locked length, holes, own-element shadowing, inherited nonwritable/writable data,
  custom prototypes and creation refusal with no inherited setter.
- The same source passes Node script execution. Executed as an owned raw script fixture,
  it fails upstream `0c067611` at `strict inherited refusal`, and passes the candidate.
  Raw script mode is intentional: module execution would make the sloppy test strict.
- `mbx test -p kiln-interpreter --lib tests::objects`: 58 passed, 1 ignored, 0 failed.
- `mbx test -p kiln-conformance --lib`: 73 passed, 0 failed.
- Capped pinned Test262 comparisons retain exact case IDs and outcomes across 978 variants:

| Subtree | Variants | Upstream pass/fail | Candidate pass/fail |
| --- | ---: | ---: | ---: |
| built-ins/Array/prototype/push | 48 | 42/6 | 42/6 |
| built-ins/Array/prototype/unshift | 44 | 40/4 | 40/4 |
| built-ins/Array/prototype/reverse | 36 | 34/2 | 34/2 |
| language/expressions/assignment | 850 | 815/35 | 815/35 |

The comparison authority is the immutable latest-upstream `0c067611` executable:
`~/Developer/kiln-slow-case-baseline/.scratch/slow-case-baseline/kiln`, SHA-256
`e51323c8eca1cf274725c10b9cd970a4b2f09df14ffeb9ba9cd067d5bdf7c157`.
The candidate includes the integrated optimization bundle plus this correction; this
comparison is not an isolated performance claim. Test262 uses the original pinned corpus,
4096 MiB cap, four-worker allowance and unchanged execution limits via
`scripts/test262_corpus.run_result`. Full logs remain locally under
`docs/issues/array-prototype-writes/evidence/`. Root owns the final combined gate.
