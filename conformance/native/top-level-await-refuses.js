// A module that awaits at its top level, which this backend refuses rather than compiling into
// C that does not build.
//
// WHY A PROBE FOR SOMETHING THAT REFUSES. The entry point runs once on an automatic frame and
// has nowhere to suspend; a generator keeps a frame for that, and the entry point does not.
// Until 2026-09-05 a top-level `await` reached the C compiler as an undeclared `frame` and the
// build failed there, naming nothing the program could act on. The decision is to refuse, by
// name, until the entry point is given a frame of its own. If somebody gives it one, this
// probe starts *matching* and that is the right day to delete it.
console.log("before");
const value = await Promise.resolve(7);
console.log("after", value);
