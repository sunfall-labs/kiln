// Reading a name a module namespace does not export is the language's own answer: a namespace
// is an exotic object whose own keys are exactly its exports, so [[Get]] of any other key is
// `undefined` and [[HasProperty]] is false. Only the import *declaration* `import { missing }`
// is a link-time SyntaxError. Bundled libraries feature-test optional exports this way (Effect
// reads `Scheduler.defaultScheduler`), and the native build used to fail the whole graph with
// "imports missing export" when every read of a namespace was a constant member name.
//
// The native backend plans a namespace from its readers: when every use is a constant member
// read it builds a partial object from those names, and any other use (destructuring, `typeof`,
// `in`, a computed key, dynamic import) gets the complete one. `ns` and `re` below are read by
// constant names only -- the path that failed -- and `shapes` takes the other uses. This probe has
// no dynamic import, which would make every namespace complete;
// `module-namespace-absent-member-dynamic.mjs` covers that.
import * as ns from './fixtures/module-namespace-absent-member-static.mjs';
import * as re from './fixtures/module-namespace-absent-member-barrel.mjs';
import * as shapes from './fixtures/module-namespace-absent-member-shapes.mjs';

// Constant member names only: the partial-namespace path.
console.log(ns.a, ns.missing, ns['missing']);
console.log(ns.missing === undefined, ns.missing ?? 'fallback', ns.missing?.deep);
console.log(ns.present());
if (ns.missing) console.log('truthy');
else console.log('falsy');

// A namespace re-exported through `export * as`, read by constant member names.
console.log(re.outer, re.missing, re.missing ?? 'barrel-fallback');
console.log(re.inner.b, re.inner.missing);

// The complete-namespace shapes.
console.log(shapes.e, typeof shapes.missing, 'missing' in shapes, 'e' in shapes);
console.log(typeof shapes.missing === 'function' ? 'has' : 'lacks');
const { e, missing, absent = 'default' } = shapes;
console.log(e, missing, absent);
