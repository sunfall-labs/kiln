# PRD: Fix Native Inline Method Call Site Cache Collisions

Status: resolved
Branch: `fix/inline-method-cache-shape`

## Problem Statement

Following the landing of scalar replacement (commit `a5aa5447`), three third-party bundles diverged natively. The inline method call site cached callee values keyed only by the receiver's shape without checking prototype identities.

## Resolution

- Fixed shape cache keying in inline call sites to validate prototype method identity.
- Verified all 19 third-party bundles pass cleanly under `scripts/thirdparty.sh`.
