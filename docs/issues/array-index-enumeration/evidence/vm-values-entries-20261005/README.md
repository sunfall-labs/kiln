# Interpreter values and entries descriptor walk

Object.values and Object.entries snapshot own keys, check each current own descriptor and read through [[Get]]. The ordinary receiver path checks metadata without constructing descriptor objects. The Proxy path preserves its observable ownKeys/getOwnPropertyDescriptor/get operations and excludes symbols and non-enumerable properties. Earlier getters can expose, hide or remove later keys.

The two regressions check an array getter deleting a later element and Proxy descriptor/symbol filtering. The affected interpreter crate passed 281 tests with zero failures and one ignored using pinned Rust 1.97.1, one worker and one test thread. Complete raw output, terminal status, command/environment and validated source hashes are retained here. Combined CLI/native parity for the committed mutation fixture is the next verification step; this receipt does not claim that step has run.
