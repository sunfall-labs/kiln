This crate vendors two number-conversion components for Kiln's emitted C runtime.

## Ryu formatting

The binary64-to-shortest-decimal formatter comes from Ulf Adams' Ryu C implementation.

- Repository: https://github.com/ulfjack/ryu
- Commit: `4c0618b0e44f7ef027ebae05d2cc7812048f7c8f`
- Selected license: Boost Software License 1.0 (see `LICENSE-Boost`)
- Kiln changes: amalgamated the required headers and `d2s.c`, made the Ryu entry point private
  and namespaced, and added `kiln_number_to_js_string` for ECMAScript presentation thresholds.

## fast_float C parsing

The correctly rounded, bounded decimal parser is the single-header C99 port of fast_float.

- Repository: https://github.com/kolemannix/ffc.h
- Commit: `f0fb776c85bedd41339121caac6caaddf370f3d6`
- Vendored file: `c/ffc.h`
- SHA-256: `1b038918337bf4ab90ac73dd6afb79ca375312f2fcbe2f64ad80ec9e4aa34fd9`
- Selected license: Boost Software License 1.0 (the file is also offered under MIT or Apache-2.0)
- Kiln changes: none in `ffc.h`; the adjacent adapter owns ECMAScript grammar, Unicode
  whitespace, `Infinity`, and arbitrary-length binary/octal/hex integer conversion.

When refreshing either source, update its commit and checksum above, retain upstream notices, run
the crate's strict C11 tests, and intentionally bump the corresponding conversion version.
