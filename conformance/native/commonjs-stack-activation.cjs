const previous = Error.prepareStackTrace;
Error.prepareStackTrace = (_error, sites) => sites;
console.log(require('./fixtures/commonjs-stack-initializer.cjs'));
Error.prepareStackTrace = previous;
