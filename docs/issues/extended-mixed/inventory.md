# All 48 mixed extended files

Times are summed sloppy/strict milliseconds in the same 4-worker, 4096MiB batch. Shared-machine contention makes these diagnostic, not stable benchmark estimates. Exact variant IDs and outcomes matched; focused repeated measurements are in README.md.

| File | Before ms | After ms | Outcomes | Cost / remaining work |
|---|---:|---:|---|---|
| `annexB/built-ins/RegExp/RegExp-leading-escape-BMP.js` | 1949.28 | 2000.86 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `annexB/built-ins/RegExp/RegExp-trailing-escape-BMP.js` | 1987.95 | 2019.1 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `built-ins/Array/prototype/every/15.4.4.16-7-c-ii-2.js` | 441.75 | 470.88 | pass, pass | Sparse array at index 999999: 1M HasProperty checks; avoidable dense hole traversal needs prototype guards. |
| `built-ins/Array/prototype/map/15.4.4.19-8-c-ii-1.js` | 457.56 | 508.14 | pass, pass | Sparse array at index 999999: 1M HasProperty checks; avoidable dense hole traversal needs prototype guards. |
| `built-ins/Iterator/zip/basic-longest.js` | 932.61 | 1188.07 | pass, pass | Combinatorial iterator input/padding matrix; allocation, callbacks and result assertions. |
| `built-ins/Iterator/zipKeyed/basic-longest.js` | 1084.57 | 1405.94 | pass, pass | Combinatorial iterator input/padding matrix; allocation, callbacks and result assertions. |
| `built-ins/decodeURI/S15.1.3.1_A1.12_T1.js` | 643.68 | 765.44 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A1.12_T2.js` | 667.5 | 773.6 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A1.12_T3.js` | 642.03 | 756.99 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A1.2_T1.js` | 574.54 | 638.67 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A1.2_T2.js` | 584.33 | 643.47 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A2.1_T1.js` | 571.6 | 605.31 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURI/S15.1.3.1_A2.4_T1.js` | 1109.61 | 1250.73 | pass, pass | Exhaustive three-byte UTF-8 decoding (16*64^2 candidates), hex construction and assertions. |
| `built-ins/decodeURI/S15.1.3.1_A2.5_T1.js` | 2112.78 | 2347.79 | fail, fail | Exhaustive four-byte UTF-8 decoding (5*64^3 candidates); finite sweep exceeds 8M execution steps. |
| `built-ins/decodeURIComponent/S15.1.3.2_A1.12_T2.js` | 702.05 | 727.72 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURIComponent/S15.1.3.2_A1.12_T3.js` | 648.46 | 702.35 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURIComponent/S15.1.3.2_A1.2_T1.js` | 581.1 | 629.06 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURIComponent/S15.1.3.2_A1.2_T2.js` | 583.6 | 635.97 | pass, pass | BMP/invalid-escape URI decoding sweep; conversion, validation and JS loop work. |
| `built-ins/decodeURIComponent/S15.1.3.2_A2.4_T1.js` | 1108.0 | 1167.02 | pass, pass | Exhaustive three-byte UTF-8 decoding (16*64^2 candidates), hex construction and assertions. |
| `built-ins/decodeURIComponent/S15.1.3.2_A2.5_T1.js` | 2127.76 | 2227.53 | fail, fail | Exhaustive four-byte UTF-8 decoding (5*64^3 candidates); finite sweep exceeds 8M execution steps. |
| `built-ins/encodeURI/S15.1.3.3_A2.3_T1.js` | 1437.14 | 1538.11 | pass, pass | BMP three-byte UTF-8 encoding sweep; hex construction and assertions. |
| `built-ins/encodeURIComponent/S15.1.3.4_A2.3_T1.js` | 1425.37 | 1538.19 | pass, pass | BMP three-byte UTF-8 encoding sweep; hex construction and assertions. |
| `intl402/DateTimeFormat/prototype/formatToParts/compare-to-temporal.js` | 826.02 | 913.49 | pass, pass | Calendar/year/month product: DateTimeFormat/Temporal conversions and field comparisons. |
| `intl402/Temporal/ZonedDateTime/prototype/equals/canonical-not-equal.js` | 694.05 | 772.44 | fail, fail | All timezone pairs; existing correctness failure: Etc/GMT and Etc/UTC both returned as canonical IDs. |
| `language/comments/S7.4_A5.js` | 1499.84 | 1541.19 | fail, fail | BMP dynamic comment parsing; existing checked-source refusal on unpaired surrogates. |
| `language/identifiers/start-unicode-10.0.0-class.js` | 388.42 | 9.24 | pass, pass | Large generated private-field class: parsing and constant interning; improved compiler index applies. |
| `language/literals/regexp/S7.8.5_A1.1_T2.js` | 1981.51 | 2016.17 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `language/literals/regexp/S7.8.5_A1.4_T2.js` | 1822.8 | 2126.36 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `language/literals/regexp/S7.8.5_A2.1_T2.js` | 1928.87 | 2336.56 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `language/literals/regexp/S7.8.5_A2.4_T2.js` | 1859.77 | 2130.62 | fail, fail | Exhaustive BMP dynamic-regexp parsing; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-1-of-8.js` | 655.21 | 769.43 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-2-of-8.js` | 665.17 | 744.93 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-3-of-8.js` | 655.63 | 721.77 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-4-of-8.js` | 672.39 | 723.54 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-5-of-8.js` | 662.5 | 729.46 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-6-of-8.js` | 652.54 | 722.0 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-7-of-8.js` | 651.76 | 725.0 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/dst-offset-caching-8-of-8.js` | 653.14 | 722.94 | fail, fail | Four nested timestamp loops for DST cache stress; finite sweep exceeds 8M execution steps. |
| `staging/sm/Date/two-digit-years.js` | 730.77 | 802.26 | pass, pass | 100*12*31 Date parse combinations plus 1000 year cases; substantial necessary validation work. |
| `staging/sm/Function/has-instance-jitted.js` | 362.68 | 419.52 | pass, pass | Repeated instanceof and bound/custom hasInstance calls; JIT warmup loops executed by interpreter. |
| `staging/sm/Proxy/ownkeys-linear.js` | 4764.53 | 2913.28 | pass, pass | 15k keys in reverse order: removed quadratic duplicate/membership scans; ordinary property definition/descriptor lookup remains linear per key. |
| `staging/sm/RegExp/unicode-class-braced.js` | 909.93 | 1052.99 | pass, pass | 16MiB zero-padded Unicode escape parsed twice, via eval and RegExp constructor; parser/input work. |
| `staging/sm/String/string-upper-lower-mapping.js` | 22383.67 | 1417.91 | pass, pass | 65,536-entry BMP casing table; removed quadratic compiler constant interning, runtime Unicode sweep remains. |
| `staging/sm/TypedArray/set-same-buffer-different-source-target-types.js` | 1609.77 | 1773.0 | fail, fail | Overlap copy plus 524,288 element assertions; finite workload exceeds 8M execution steps. |
| `staging/sm/TypedArray/sort_sorted.js` | 499.03 | 541.58 | pass, pass | Ascending/descending arrays of lengths 0..255: compare callbacks, element stores and sorted-result assertions. |
| `staging/sm/regress/regress-1507322-deep-weakmap.js` | 293.97 | 309.95 | unsupported, unsupported | Build 99,999-node WeakMap chain; missing $262.gc host capability, so post-GC walk does not run. |
| `staging/sm/regress/regress-610026.js` | 3807.53 | 4206.98 | pass, pass | Dynamic parsing of millions of empty blocks in 4/8MiB strings; actual parser/allocation work. |
| `staging/sm/statements/for-of-iterator-close-throw.js` | 350.0 | 448.95 | fail, fail | Correctness bug, not inherently large: tiny return/try/finally with throwing IteratorClose loops until 8M steps. |
