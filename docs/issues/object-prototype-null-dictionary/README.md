# Null-prototype `__proto__` handling

Null-prototype dictionaries must treat `__proto__` as ordinary data: they have no
Object.prototype accessor in their chain. Ordinary objects must continue to use the
inherited accessor, including when an empty user prototype sits between the object and
Object.prototype. The VM now selects the synthetic accessor only when the actual chain
reaches Object.prototype and no own/custom stored property shadows it.

The regression covers absent reads, first assignment, a subsequent data update, an
intermediate prototype's own data property and getter-only accessor, and the ordinary-object
inherited accessor control. Validation on the frozen candidate source:

- Focused interpreter regression: 1 passed.
- DecisionModel and FindMyWay `params-collisions`: 95/95 passed, no oracle mapping or
  outcome mismatches.
- Test262 `built-ins/Object/prototype`: the same 494 IDs as baseline; 464 pass and 30
  fail on both, with zero pass-to-fail regressions and zero gains.
- `kiln-interpreter`: 261 passed, 0 failed, 1 ignored.

The compressed evidence archive contains the exact reports and raw unit log. Candidate CLI
SHA-256 is `518321685f3b3d2b2bd3c74262cceb2b40daa37f7765b8a122b5100cb8f02651`; it was built
from base commit `c4d033237dc05c8d23592d1548e9bcccbfe35cf1` plus the production source diff whose
SHA-256 was `7b702a79ff2c747926bf39d12988e0fbac05d000fec5c3d2bf414daa917d3b21`. The Test262 baseline
CLI was the pushed integration binary with SHA-256
`d0830baef65b01c9ccfeb626984e44cf863418a7008596188feb29822949ea88`.

The 494-case baseline and candidate reports were compared by exact case ID and outcome. The
baseline report digest is `a327b9c5ee26678531660fdd55bef4516bd8248f56ff30207029fbbb02f3ecbb`;
the candidate report digest is `e898b9f379875f0e9775682fbf7e08aa33926e2f3ed918bd5d04d04213b4b3c4`.
All archive member digests are listed in `evidence/SHA256SUMS` inside
`evidence-20261005.tar.zst`.
