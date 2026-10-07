# Computed-call canonical-key validation

This evidence validates the ordinary, nonspread computed-call correction on source base
`d3b28c1fe0d6dd9968e9e5035843b98b83380449` in the isolated `fix/computed-call-prepared`
worktree. The compiler now executes `ToPropertyKey` once before `GetComputed`, retaining the
canonical key beside the receiver and captured callee. This gives the invocation the converted
string method alias without repeating a user-defined conversion. The existing fixed-arity
`CallComputed` path continues to use its stack argument buffer, and `GetComputed` remains a graph
operation so symbol reads keep using the existing exact-key symbol PIC.

The fixture checks `Symbol.toPrimitive` → getter → argument ordering, receiver identity, a
canonical method alias from an object key, nullish receiver conversion suppression, argument
suppression when the getter throws, and argument evaluation before a noncallable TypeError. Existing
fixture cases also cover alternating symbol keys, proxy reads and receiver, mutation during
argument evaluation, and captured methods. Spread and optional calls remain on their existing
`Apply` path; this evidence makes no claim about method-alias metadata for those paths.

Validation used Node `v26.8.1`; Node and the candidate interpreter outputs match byte-for-byte.
`scripts/native.sh conformance/native/computed-method-call-pic.js` reports `1 matched, 0 refused,
0 diverged` using the candidate CLI and the fixture's original default profile. Its interpreted
output agrees with Node. The focused IR test
`computed_method_calls_convert_once_and_resolve_before_arguments` passed (1 passed).
`cargo fmt --all -- --check` and `git diff --check` passed.

The candidate CLI is `target/release/kiln`, SHA-256
`d53744a78cbd3bc9d155b968a6510464635f8b76786b54e064b880ea4c9c6bbc`. It was built with Rust
1.97.1 through `mbx build --jobs=1 -p kiln-cli --release`; source code was the worktree diff from
the base commit above. The final fixture bytes have SHA-256
`1a13ce5fecf0a5d1437c540c84ee23b298a5fdeb3fa08375857360da14e1dd1d`. The captured Node and
interpreter outputs are identical (SHA-256 `60510bc0a34f147eb083f4a4b7215659928356059d4a0ecc36362c51666de272`).

`native-default.log`, `node.out`, and `interpreter.out` retain the final matching run. An earlier
native-release run happened before the stack-alias assertion was corrected to follow Node's
CallSite behavior; it is exploratory only and is not included as validation evidence.
