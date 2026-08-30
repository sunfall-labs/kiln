// An error's own properties are readable by name and never *enumerated*: `err.message` works,
// `Object.keys(err)` is empty, and `JSON.stringify(err)` is `{}`. kiln enumerated them, so any
// program that serialised an error or spread one got fields node omits -- and `Object.keys` and
// `Object.getOwnPropertyNames` were the same function, which they are not: one reports what is
// enumerated and the other reports everything.
const e = new Error("boom");
console.log(Object.keys(e), Object.entries(e), Object.values(e), JSON.stringify(e));
console.log({ ...e }, Object.assign({}, e), JSON.stringify({ wrapped: e }));
const seen = [];
for (const key in e) { seen.push(key); }
console.log(seen);

// Reading still works, which is the whole point of the distinction.
console.log(e.message, e.name, String(e), e instanceof Error, "message" in e);
const typed = new TypeError("bad");
console.log(typed.message, typed.name, String(typed), typed instanceof TypeError, typed instanceof Error);

// An ordinary object is unaffected: everything it has is enumerated.
const plain = { a: 1, b: 2 };
console.log(Object.keys(plain), Object.getOwnPropertyNames(plain), JSON.stringify(plain));
console.log({ ...plain }, Object.entries(plain), Object.values(plain));

// An error carrying extra fields: the ones the program added are enumerated, the built-in ones
// are not, which is how a structured error survives `JSON.stringify`.
const detailed = new Error("failed");
detailed.code = 42;
detailed.detail = { why: "because" };
console.log(Object.keys(detailed), JSON.stringify(detailed), detailed.message);
