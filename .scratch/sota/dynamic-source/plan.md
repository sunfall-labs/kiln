# Dynamic source boundary plan

This is a review artifact, not an implementation.

## Existing contract

`kiln-interpreter::EvalSource.source` and `SourceCompiler` carry `&str`. The dynamic entry points in `crates/interp/kiln-interpreter/src/lib.rs` currently call `StringUnits::to_lossy_string()` before that boundary. Oxc's `Parser`, `Lexer::Source`, AST `StringLiteral`/`TemplateElement`, and spans all operate on a valid UTF-8 `&str`; spans are byte offsets into that same text. Oxc has no UTF-16 source constructor in the pinned 0.147 APIs.

Oxc's existing escaped-surrogate support is sufficient for well-formed source text: the lexer marks escaped literals with `lone_surrogates`, stores a reversible U+FFFD-plus-hex sentinel in `Str`, and `kiln-frontend-oxc::oxc_string_units` restores UTF-16 units. This covers `eval("\\uD800")`, escaped template contents, and escaped property keys without a dynamic source rewrite.

## Bounded implementation shape

1. Add a single checked conversion at the interpreter/source-compiler boundary. It accepts `StringUnits`, rejects any unpaired UTF-16 unit with `EvalCompileError::Unsupported("dynamic source contains unpaired surrogate code units")`, and hands a valid scalar `&str`/owned string to Oxc. Do not inspect source context or rewrite comments, regexps, templates, or literals.
2. Keep ordinary ASCII source on a borrowed fast path. For non-ASCII but scalar units, use an ecosystem conversion (`String::from_utf16` or an equivalent shared `StringUnits` scalar view) once. This is a source adapter, not a second lexer.
3. Route DirectEval, indirect eval, HostEvalScript, and ShadowRealm.evaluate through that one boundary. Function/Generator/AsyncGenerator constructors should assemble their parameter/body ToString results as `StringUnits`, preserve those units in the generated `Program.source`, then use the same checked boundary.
4. Keep `EvalCompileError::Syntax` for parser diagnostics. A raw malformed UTF-16 source refusal is `Unsupported`, so user code can distinguish an implementation boundary from a genuine syntax error. Escaped `\\uD800` outside a literal remains a parser SyntaxError after the normal Oxc path.
5. Keep `Program.source` as the source spelling actually handed to the parser, so spans and Function.prototype.toString remain aligned. No source rewriting means Oxc byte offsets remain valid.

## Deliberate unsupported case

A String value containing a raw lone surrogate anywhere in dynamic source is refused, including a raw surrogate in a quoted literal, regexp, comment, or tagged-template raw segment. Supporting only some contexts would require a complete contextual lexer (regex/division, template substitution, comments, source locations) or a UTF-16 parser; doing so with a partial rewrite would silently change source semantics. A future parser API can widen this boundary without changing the caller semantics.

## Other boundary

`ShadowRealm.importValue` export names are a separate lookup contract. Since static module export names are validated well-formed Unicode, lookup must compare checked units or reject lone-surrogate names; it must not use lossy replacement and alias U+FFFD.

## Expected files if implemented

- `crates/interp/kiln-interpreter/src/lib.rs`: one source conversion helper and the five dynamic call sites/function source assembly.
- `crates/interp/kiln-interpreter/src/tests.rs`: escaped source success, raw-unit explicit refusal, escaped invalid-outside-literal SyntaxError, and Function source spelling/reflection.
- `crates/foundation/kiln-string/src/lib.rs`: only if the existing shared owner lacks a borrow-first scalar view; add a small `with_scalar_str`/checked conversion API rather than a second string type.
- `crates/syntax/kiln-frontend-oxc/src/lib.rs`: no parser rewrite; retain and test the existing Oxc sentinel decoder.
