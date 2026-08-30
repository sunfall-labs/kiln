This crate vendors the binary64-to-shortest-decimal portion of Ulf Adams' Ryu C implementation.

- Repository: https://github.com/ulfjack/ryu
- Commit: `4c0618b0e44f7ef027ebae05d2cc7812048f7c8f`
- Selected license: Boost Software License 1.0 (see `LICENSE-Boost`)
- Kiln changes: amalgamated the required headers and `d2s.c`, made the Ryu entry point private
  and namespaced, and added `kiln_number_to_js_string` for ECMAScript presentation thresholds.

When refreshing the vendored source, update the commit above, retain upstream notices, run the
crate's strict C11 tests, and intentionally bump `NUMBER_FORMAT_VERSION`.
