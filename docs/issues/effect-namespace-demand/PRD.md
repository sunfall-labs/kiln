# Escaping namespace demand

Status: ready-for-agent

An already visited provider could retain only a static export subset when a later import escaped its full namespace. The worklist inserted the broader demand but queued only newly live modules, leaving the provider’s other exports removed. Native upstream TxHashSet consequently passed 5 of 30 tests; 25 called missing HashSet exports.

Requeue on either a newly live provider or a newly demanded full namespace. The eight-case regression matrix covers both import orders, direct and reexported static namespace reads, and direct and reexported escaping namespace reads. The kiln-liveness crate passes all five tests. The unchanged upstream TxHashSet file now passes all 30 native-release tests without skips. This is targeted validation, not a complete Effect or gate result.

`reference.json` binds the retained pre/post logs, source, compiler, executable and provider identities. The upstream run used a fresh daemon created by the frozen compiler; final integrated survey runs use `--one-shot`.
