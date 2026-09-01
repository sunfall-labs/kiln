// Annex B B.3.2.1 and B.3.3.2: a block-level function declaration in sloppy code also declares a
// `var` of the same name in the enclosing function or script, written as the declaration is
// evaluated. Legacy semantics that bundled JavaScript still relies on, and sloppy-only -- hence
// a `.cjs`, the only entry either engine compiles as sloppy code.
console.log('before:', typeof f);
{
  function f() {
    return 'declaration';
  }
}
console.log('after:', typeof f, f());

// The two bindings are independent once the copy has happened.
var initial, current, outer;
(function () {
  {
    function g() {
      initial = g;
      g = 123;
      current = g;
      return 'decl';
    }
  }
  outer = g;
  g();
}());
console.log('split:', initial(), current, outer());

// A lexical declaration of the same name means replacing the declaration with a `var` would be
// an early error, and Annex B declines exactly there.
var lexical = (function () {
  let h = 123;
  {
    function h() {}
  }
  return h;
}());
console.log('lexical:', lexical);

// A formal parameter and `arguments` are the call's own bindings and are never taken over.
console.log('parameter:', (function (p) {
  {
    function p() {}
  }
  return p;
}(123)));
// `arguments` is appended to parameterNames whenever the object is needed, so B.3.2.1 declines
// there for the same reason it declines for a formal. Node overwrites the object here and fails
// test262's `block-decl-func-skip-arguments.js`; both kiln engines follow the specification, so
// this line is one of the few in this corpus that is deliberately not what node prints.
console.log('arguments:', (function () {
  {
    function arguments() {}
  }
  return arguments.toString();
}()));

// `if (x) function f() {}` is the same rule reached through B.3.4: the arm is read as though a
// block were written around it, so the declaration binds there and only the Annex B write
// reaches the enclosing scope -- which is why the parameter below survives.
if (true) function branch() { return 'branch'; }
console.log('if-branch:', branch());
console.log('if-branch parameter:', (function (p) {
  if (true) function p() {}
  return p;
}(123)));

// A switch clause is a block for this purpose too.
switch (0) {
  default:
    function inClause() { return 'clause'; }
}
console.log('switch:', inClause());
