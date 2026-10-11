# `typeof` of a deleted built-in global throws

Status: closed

Both kiln engines throw a ReferenceError for `typeof` of a built-in global the program deleted;
the language answers `"undefined"` for an unresolvable reference (ECMA-262 §13.5.3.1 step 2).

```js
delete globalThis.JSON;
console.log(typeof JSON);   // node: undefined   kiln (interpreter and native): ReferenceError
```

Found by `conformance/native/global-read-hint-sites.mjs`, which checks the deleted name with
`in` instead until this is fixed.

Fixed: `typeof X` for a built-in name compiles to the run-time global lookup every unbound name
uses (`LoadGlobalBindingOrUndefined`), so both engines answer "undefined" once it is deleted.
The probe checks it again.
