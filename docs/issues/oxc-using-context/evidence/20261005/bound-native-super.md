# Bound native superclass adoption

The initial Proxy-super change preserved Proxy construct results by limiting prototype transfer
to native builtin superclasses. Bound constructors are unwrapped by `kiln_c99_super_call` before
dispatch, so a bound native builtin also needs the native-adoption path. The adoption helper now
classifies through bound targets but deliberately does not unwrap Proxies: Proxy construction
already forwards the derived `new.target` and returns the correct object.

The fixture adds `class BoundArrayDerived extends Array.bind(null) {}` and verifies that its
instance is an Array, is an instance of the derived class, and retains initialized array elements.
The direct Node oracle passed.

Validation used the one-worker release CLI built from the follow-up source. CLI SHA-256:
`505edd21af8deb31be3cc9859e1102e37b977959101328faebfd34fd68d9ce42`. The source/test diff against
`50e94d52e70a60ba4f40dab021db1e2b3fad1cf1` had SHA-256
`1f17662a97d67a16207a4ed260b5667f2e2fa1aca2cccbc9945fd3e43316f65f`.

- Ordinary native parity: `1 matched, 0 refused, 0 diverged`.
- Native GC stress: `1 matched, 0 refused, 0 diverged`; no stress probes skipped.
- Raw outputs are retained in `native-bound-super.log` and
  `native-bound-super-stress.log`.
