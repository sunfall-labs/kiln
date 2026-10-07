# Borrowed hasOwnProperty authority

The frozen `4f39b9dc` gate exposed `Object.prototype.hasOwnProperty.call(generator, "next")`
answering true, although `Object.hasOwn` and Node correctly answered false. The generic exotic
fallback asked inherited membership. Every receiver now uses the same own-descriptor authority
as `Object.hasOwn`.

CLI SHA-256: `77597bd4070db6191813df77881a89a3e00ef0a2b63f192681ff0c6a3126b87a`.

Validation:

- Expanded existing native probe: one matched, zero refusals or divergences. It covers sync and
  async generators, inherited collection methods, symbols, and an own undefined value.
- The interpreter's complete probe output matches Node's exact bytes.
- Pinned Test262 `built-ins/Object/prototype/hasOwnProperty`: all 126 variants passed.
- Formatting and diff checks passed.

Compressed outputs and Test262 case outcomes are preserved here. The routine gate is pending
on this source correction; the earlier failed gate remains archived without changing its verdict.
