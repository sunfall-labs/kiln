// `process.argv` in a compiled binary follows node's single-executable-application convention:
// slots 0 and 1 both name the executable, because there is no separate interpreter to name and
// no script file to name, and the program's own arguments still begin at index 2. The
// interpreted run names the interpreter and then the entry -- node's other spelling of the same
// shape. What the two leading strings *are* therefore differs between the engines, and this
// probe deliberately asserts none of it. What must agree is the shape every argument-reading
// program depends on: two leading entries, both strings, and nothing after them when nothing
// was passed on the command line.
console.log(process.argv.length >= 2);
console.log(typeof process.argv[0]);
console.log(typeof process.argv[1]);
console.log(JSON.stringify(process.argv.slice(2)));
console.log(process.execPath === process.argv[0]);
