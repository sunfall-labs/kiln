# `postcss` diverges natively: `require("path").dirname` is undefined

Status: resolved. `postcss` matches node byte for byte through both engines.

> This file arrived on `main` carrying unresolved merge-conflict markers -- two sides of a merge
> committed verbatim. Resolved here in favour of the diagnosis, which is the half this issue is
> about. The other half was a `vue-reactivity` note that already has its own issue as 06.

It was the only *wrong answer* in the corpus, and therefore the most serious item here.

    node:   nodes 18|types atrule:4,comment:1,decl:9,rule:4|...
    native: uncaught TypeError: a undefined is not a function
            at instruction=878 operation=CallMethod(parse) detail=CallMethod invocation threw
            receiver_tag=5

## What it actually is

`receiver_tag=5` is `postcss` itself and `parse` is *there*: the diagnostic says "invocation
threw", not "lookup refused", and the whole earlier line of enquiry -- which shape of property
assignment onto a function loses the property -- was chasing a member that is not missing. Bisected
by cutting the bundle at its last import and instrumenting inward:

    postcss.parse(css)
      -> parse2(css, opts)
        -> new Input(css, opts)          <- throws here
          -> new PreviousMap(css, opts)
            -> dirname(this.mapFile)     <- `dirname` is undefined

The native `node:path` namespace publishes the *interpreter's* full export-name list and fills
only the five names the native side implements -- `isAbsolute`, `join`, `resolve`, `sep`,
`default`. `dirname`, `relative`, `basename`, `extname`, `normalize`, `format`, `parse`,
`delimiter` and `posix` are all present, all enumerable, and all read `undefined`. `node:fs` does
the same: it fills `existsSync` and `promises` and publishes `readFileSync` and `realpathSync`
beside them.

The two halves of the loader disagree about this. A static `import { dirname } from "node:path"`
refuses by name -- "native Node execution does not yet materialize the `node:path` export
`dirname`" -- while `require("path").dirname`, which is what an esbuild bundle emits, answers
undefined and lets the program run on. `postcss` is built to work without `path`, but it asks the
wrong question: `input.js` computes `pathAvailable` from `resolve && isAbsolute`, both of which the
native side *does* have, so it is true and the constructor goes on to build a `PreviousMap` --
and `previous-map.js` destructures `dirname` from the same module with no guard of its own.

Reduced to a runnable eleven lines in
`conformance/thirdparty/findings/node-builtin-exports-read-undefined.cjs`.

## What was done

`path.dirname`, `basename`, `extname` and `relative` are implemented natively against the
interpreter's arithmetic -- the interpreter is the oracle here, and it is not always node -- and so
is `String.fromCharCode`, which was the second thing this bundle needed and was in
`builtin_backlog!`.

A published member with nothing behind it is now a **callable that refuses by name when it is
called**, rather than a name that reads `undefined`. Refusing on the *read* was tried first and is
wrong: `const { existsSync, readFileSync, realpathSync } = require("fs")` is `previous-map.js`'s
opening line, and destructuring reads every name it mentions whether or not the program calls one,
so a refusal there stops a program at its first import over a function it never uses. Refusing on
the call also leaves `typeof fs.readFileSync === "function"` agreeing with the interpreter, which
is what a feature test asks. `path.posix` is the exception and refuses on the read, because it is
the module itself in the interpreter and no value this backend can hand back has that identity.

Two further silent wrong answers turned up in the same arithmetic and are fixed with it:

- the native normalizer dropped a trailing separator the interpreter keeps, so
  `path.join("a", "b/")` answered `"a/b"` against `"a/b/"`, and `path.resolve` with it;
- `os.EOL` and `os.devNull` read `undefined` against the interpreter's `"\n"` and `"/dev/null"`.

`conformance/native/node-path-members.cjs` covers the members and
`node-path-unbuilt-members-refuse.cjs` the refusal.

## Still open here

`path.parse`, `path.format` and `path.normalize` refuse rather than answer. Each wants a method id
and nothing else -- the C body `normalize` needs is already in the file, under `join` and
`resolve`. `path.posix` wants the shared ABI to be able to say "this export is the module itself",
which is one field it does not have; without it the ESM namespace and the CommonJS projection are
two different objects and neither has the identity the interpreter gives.

## Where `postcss` stops now

Six of its nine lines match the interpreter byte for byte -- the whole parse, transform and
stringify pipeline. It then **refuses**, which is a different outcome from diverging:

    kiln: native backend refusal: operation=SpreadObject

The remaining three lines are behind `postcss([plugin]).process(out).then(...)`, and `LazyResult`'s
constructor opens with `this.helpers = { ...postcss2, ... }` where `postcss2` is a *function*.
Spreading a function refuses natively -- measured directly, on a three-line probe, and nothing to
do with `node:path`. Spreading a Proxy refuses too, and the same pipeline reaches one through
`toProxy()`. That is the next item for this bundle and it wants an issue of its own.
