# Completed-generator cleanup validation

The completed-generator fix releases execution-only locals, operands, arguments and continuation state after normal completion, return or throw. It preserves generator properties, escaped captures, completion values and thrown-value identity. Native parity and stressed parity pass the focused `generator-completed-local-gc.mjs` fixture. The async fixture additionally covers throw and object return values.

This is an independent lifetime fix. The unchanged upstream interpreter Pool file still reports **44 passed / 3 failed / 0 skipped**, with all 47 case IDs mapped to the official Node/Vitest reference. The failures remain invalidated minimum-size resources, asynchronous background cleanup failures, and shutdown while the Pool remains reachable. Cleanup does not resolve these three Pool cases.

The retained Pool report used source `bec09b5987` and CLI SHA-256 `6c61abc10268952c635cd40c3c8f2160654d9758e74361d19ac34e3a972b9405`. Its pinned test-source SHA-256 is `a219193dd30dc547f7364a723e2682d653ab824ccbda816b48b1a9d5bfb539f8`. The aggregate, case document and raw log are retained here. This is focused evidence, not complete Effect coverage.
