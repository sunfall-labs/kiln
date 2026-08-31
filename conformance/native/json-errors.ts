// Native builtins must raise ordinary JavaScript errors, so a try/catch can handle them and the
// enclosing program continues after the failed operation.
try {
  JSON.parse('{oops');
  console.log('parse:missing');
} catch (error: any) {
  console.log('parse', error instanceof SyntaxError, error.constructor.name);
}
console.log('after-parse');

const cycle: any = {};
cycle.self = cycle;
try {
  JSON.stringify(cycle);
  console.log('stringify:missing');
} catch (error: any) {
  console.log('stringify', error instanceof TypeError, error.constructor.name);
}
console.log('after-stringify');
