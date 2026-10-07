// Diagnostic-only sink. Values remain numbers; never retain test/runtime objects.
const counters = Object.create(null)
globalThis.__KILN_SHARDING_DIAGNOSTIC__ = counters
