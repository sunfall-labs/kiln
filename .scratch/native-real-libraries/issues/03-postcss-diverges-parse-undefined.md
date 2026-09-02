# `postcss` diverges natively: a function's `.parse` is undefined

Status: needs bisect

The only *wrong answer* in the corpus, and therefore the most serious item here.

    node:   nodes 18|types atrule:4,comment:1,decl:9,rule:4|...
    native: uncaught TypeError: a undefined is not a function
            at instruction=878 operation=CallMethod(parse) detail=CallMethod invocation threw
            receiver_tag=5

`receiver_tag=5` is a function, so some function's `.parse` is present interpreted and absent
natively. Ruled out already, each verified as matching:

- direct property assignment onto a function (`postcss2.parse = parse2`, which is the shape the
  bundle actually uses -- it has exactly one `Object.assign` and it is not this)
- `Object.defineProperty` with a value, and with a getter
- a computed key
- a CommonJS module whose `module.exports` is a function carrying statics, through the Node adapter

So the cause is further in, and finding it means bisecting the bundle rather than guessing at
shapes. Do that before writing any fix.
