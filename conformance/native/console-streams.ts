// `console.error` and `console.warn` go to standard error, and `console.log`/`console.info` to
// standard output, in both engines and in node.
//
// They used to refuse natively, on the grounds that the host ABI published one captured output
// operation and the interpreter put these on standard error -- so printing them to stdout would
// be the two engines disagreeing about which stream a line went to. Half of that was wrong: the
// interpreter routed `ConsoleError` to `HostOp::WriteStdout` too, and said so at its own call
// site, "until stderr receives its own versioned op".
//
// The ABI has that op now, so both engines put these on standard error and both match node.
// Note this probe cannot tell the two streams apart -- the corpus compares one captured output
// per engine -- so what it checks is that the same lines appear in the same order on both. The
// routing itself is checked by running the file with the streams redirected separately.

console.log("log", 1);
console.error("error", 2);
console.warn("warn", 3);
console.info("info", 4);

// Reached as values rather than called directly, which is a separate path through the dispatch.
const write = console.error;
write("error-as-value", 5);
const shout = console.warn;
shout("warn-as-value", 6);

// Non-scalar arguments are reduced to the text a console shows, on both engines.
console.error({ a: 1 }, [1, 2], null, undefined);
