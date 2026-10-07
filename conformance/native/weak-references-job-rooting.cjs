let target = { answer: 42 };
const ref = new WeakRef(target);
target = undefined;

// Requiring this module evaluates it synchronously inside the same initial graph job. The
// allocation churn asks GC-stress runs to collect between WeakRef construction and deref.
const survivedNestedRequire = require('./weak-references-job-rooting-helper.cjs')(ref);
console.log(survivedNestedRequire);
