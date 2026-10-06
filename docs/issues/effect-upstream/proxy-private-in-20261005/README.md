# Private `#x in receiver` validation

This change gives private membership its own IR instruction (`HasPrivate`) instead of lowering it through public `in`/property access. It checks the receiver type first, then confirms both the expected class-evaluation brand and an own private member. It does not invoke accessors or Proxy public traps.

## Frozen inputs

- Source worktree: `fix/interpreter-proxy-derived-return`; implementation was tested before this receipt was added.
- Native CLI: `target/release/kiln`, SHA-256 `751ea10c0eaac8cc9f756d702b8df72e421b7be0e50f35c0f11b36fe06db4029`.
- Fixture: `conformance/native/proxy-private-in.js`, SHA-256 `c27653c710dab77ab157c8c77e5c5a0db422b40bfe78782f2d08152bc53b2195`.
- Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b` (ABI 82).
- Fixture output matched Node in interpreter, native ordinary, and native GC-stress modes. The native runner reported 1 matched, 0 refused, 0 diverged in both ordinary and stress modes.

## Checks

- `mbx test -p kiln-ir --offline -- --test-threads=1`: 74 passed.
- `mbx test -p kiln-interpreter --offline -- --test-threads=1`: 277 passed, 0 failed, 1 ignored.
- `mbx test -p kiln-graph --offline -- --test-threads=1`: 40 passed.
- `mbx test -p kiln-backend-c --lib --offline -- --test-threads=1`: 332 passed, 0 failed, 1 ignored.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

The fixture covers Proxy-local brands, no public `has` trap calls, revoked Proxy receivers, brand installation before field initialization, accessor membership without invoking the getter, separately evaluated class brands, static inheritance, primitives, and foreign receivers. Raw output is preserved in `logs/`.
