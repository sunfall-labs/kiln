# Immutable string identity and reserved-key prefix fast paths

Review source commit e79a3094 cherry-picks validated candidate 92942c76. StringUnits equality returns immediately for shared immutable same-width Arc storage; distinct allocations and mixed-width representations retain their previous code-unit comparison. Well-known symbol lookup rejects names lacking the reserved @@ prefix, with a compile-time assertion that the complete table retains that prefix.

The narrow library runs report 9 kiln-string tests and 18 kiln-builtin-surface tests passing, no failures. They include distinct allocations, shared storage, UTF-16 lone surrogates, and every well-known symbol round trip. Full raw logs and the actual validation receipt are retained. The receipt explicitly identifies uncaptured pretest source hashes and the first invocation's unrecorded process exit code; reconstructed Git blob hashes are not claimed as observed preflight evidence. No CLI, Effect run, performance gain, or full gate is proven here.

A final combined source build must rebuild or validate the provider package because kiln-string is a direct dependency of the provider. Public main remains unchanged pending integration/gate and network access.
