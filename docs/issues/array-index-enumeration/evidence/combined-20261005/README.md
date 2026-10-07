# Combined array and Proxy parity

The frozen CLI from clean tracked source `f16774178086e86a79de5a7ca7cf972325f5f745`, SHA-256 `42a6637216520ccdaaabe6795989499421fd39a311756999f860d1725db3189e`, ran six affected probes. All six matched Node and the interpreter with exit status zero. Native ordinary and `KILN_GC_STRESS=1` runs each reported six matched, zero refused and zero diverged; stress skipped zero probes.

Coverage includes canonical index boundaries and descriptor overlays, default data definitions bypassing inherited setters, getter side effects that expose/delete later enumeration keys, public Proxy descriptors/symbols, typed-array own properties, and Proxy private fields/methods/accessors. The new default-data definition fixture is recorded alongside this evidence. Full raw build and runner logs, terminal envelopes, Node/interpreter stdout/status comparisons and fixture hashes are retained.

This compiler predates the later dedicated HasPrivate opcode and lazy WeakCensus count-map optimization. Their focused evidence is retained separately. These six probes do not establish the full Effect suite or gate, and timings include compilation and concurrent machine activity rather than a quiet benchmark comparison.
