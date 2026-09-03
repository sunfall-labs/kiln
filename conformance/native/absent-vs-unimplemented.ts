// Members that used to read `undefined` in the compiled backend where the interpreter has a value.
//
// THE CLASS OF BUG THIS GUARDS. A refusal stops a program at the cause and says which member it
// lacked. An `undefined` stops nothing: `typeof x.member === "function"` takes the other branch
// and the program dies three frames later with nothing naming the member. That is the one wrong
// answer this backend is not allowed to give (ADR 0042), and a sweep of both engines on
// 2026-09-02 found thirty of them. The ones below are the ones now answered; the ones deliberately
// left refusing have probes of their own, named `*-refuses.ts`, so that "fixing" a refusal into an
// `undefined` shows up here as a divergence rather than as a quieter corpus.
//
// The value is printed, not just its `typeof`: a stand-in that is callable and answers nothing is
// the same lie one layer down.

// `String.prototype` had two members that are not operations of their own, so the generated
// dispatch group had no code for them and the read fell through to `undefined`.
console.log("String.prototype.values", typeof String.prototype.values);
console.log("String.prototype.toLocaleString", typeof String.prototype.toLocaleString);
console.log("values of", Array.from("héy".values()).join("|"));
console.log("toLocaleString of", "héy".toLocaleString());
console.log("String.prototype.values is @@iterator",
  String.prototype.values === (String.prototype as unknown as Record<symbol, unknown>)[Symbol.iterator]);

// `Atomics` is a realm object rather than a table-driven built-in, so the backlog row that makes
// its fourteen operations refuse never reached the one member that is only a name.
console.log("Atomics tag", Atomics[Symbol.toStringTag]);
console.log("Atomics toString", Object.prototype.toString.call(Atomics));

// A member the language really does put on a string, read off an instance rather than the
// prototype: the two paths share one table and must not disagree about which names exist.
console.log("instance values", typeof "ab".values);
console.log("instance toLocaleString", typeof "ab".toLocaleString);
