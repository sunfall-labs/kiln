// kiln-native: expect-exit=1
// Anything can be thrown; a value that is not an error has no stack to print.
console.log("before");
throw "just a string";
