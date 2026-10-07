const makeReader = new Function(
  "return (function (input) {\n" +
    "'use strict';\n" +
    "const { value: captured } = input;\n" +
    "function read() { return captured; }\n" +
    "return read;\n" +
  "})({ value: 'initialized after declaration instantiation' });"
);

console.log(makeReader()());
