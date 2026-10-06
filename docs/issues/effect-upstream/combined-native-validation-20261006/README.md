# Combined native semantic validation — 2026-10-06

Source `efd9e68a8920d764ccc7df1f244d7591d74794d6`, with a clean tracked tree, built successfully under Rust 1.97.1 using one-worker `mbx build --release -p kiln-cli --offline --jobs 1`. The frozen binary SHA-256 is `148946a23bb17b60b6d1f5907364c8950c752992c09949d1eeb287b3bdec4d4b`; its complete build-status receipt and source receipt are retained here.

All seven affected probes match between interpreter and native: array index enumeration, default data definitions, current-descriptor enumeration, class-field Proxy descriptors, typed-array own properties, Proxy private elements, and private membership. Ordinary and allocation-by-allocation native GC-stress runs each report **7 matched, 0 refused, 0 diverged**, with **0 stress probes skipped**. Each probe also independently exits successfully under Node and the interpreter with identical stdout and empty stderr; raw outputs and comparison receipts are retained.

These checks validate the combined fixes, including bytecode version 17 and the lazy weak census. They do not establish completion of the 322-file upstream Effect suite, performance deadlines, the repository gate, or benchmark comparisons. Durations include compilation and concurrent work, so they are not performance measurements. Concurrent-realm evidence will be recorded separately when terminal.
