# `Object.assign` refuses when the target is a function

Status: ready

Attaching statics to an exported function is one of the most common shapes in a bundle. Natively it
refuses; the same call with an object target succeeds, so this is the target check and not the
method.

    function f() { return 1; }
    Object.assign(f, { parse: () => "assign" });
    console.log(f.parse());

    interpreted  assign
    native       native backend refusal: operation=CallMethod(assign) detail=CallMethod invocation
                 refused receiver_tag=11

Control, which passes: the same two lines with `const o = { a: 1 }` as the target.

Direct assignment (`f.parse = parse`), `Object.defineProperty` with a value, an accessor, and a
computed key all already work on a function target, so the property machinery is present and only
`Object.assign`'s target handling rejects it.
