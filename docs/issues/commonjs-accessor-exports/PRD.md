# CommonJS accessor exports at ESM linking

Status: in-progress

The unchanged Effect Metric entry fails before registering tests: the interpreter reports the missing `ValueType` export from `@opentelemetry/api/build/src/index.js`. This reproduces on the integrated interpreter CLI SHA256 `b5f6b9fee0c483ff872141d01973b1e8dcc5fb14199e454696a5034fff99a21e` (source `be11cd16`), not only the earlier interrupted survey.

OpenTelemetry initializes `exports.ValueType` and then replaces it with an enumerable getter that forwards the metrics module enum. CommonJsLoader evaluates the wrapper correctly but publishes only `Property::Value` entries into its module export map. The accessor disappears before ESM linking.

The fix must observe property getters in the same realm, preserve CommonJS require identity and lazy require behavior, snapshot ESM named exports, propagate getter failures, and retain namespace identity across repeated imports. Invoking every getter whenever require evaluates a module would change behavior and is not a sufficient solution.

A minimal source regression is under development. Complete unchanged Metric execution and focused require/accessor/namespace tests must pass before this issue is complete.
