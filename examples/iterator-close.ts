function tracked(label, log) {
	let i = 0;
	return {
		[Symbol.iterator]() {
			return {
				next: () => ({ value: ++i, done: i > 3 }),
				return() {
					log.push(label);
					return { done: true };
				},
			};
		},
	};
}

// break closes
let log = [];
for (const x of tracked("break", log)) break;
console.log("break:", log.join(",") || "(none)");

// continue does not close, and running out does not close either
log = [];
for (const x of tracked("exhaust", log)) continue;
console.log("exhaust:", log.join(",") || "(none)");

// return closes
log = [];
(function () {
	for (const x of tracked("return", log)) return;
})();
console.log("return:", log.join(",") || "(none)");

// throw closes, and the exception still propagates
log = [];
try {
	for (const x of tracked("throw", log)) throw new Error("boom");
} catch (e) {
	console.log("throw:", log.join(",") || "(none)", e.message);
}

// break out of a try inside the body: the finally runs, then the iterator closes
log = [];
for (const x of tracked("nested", log)) {
	try {
		break;
	} finally {
		log.push("finally");
	}
}
console.log("nested:", log.join(","));

// a nested loop's break closes only the inner iterator
log = [];
for (const a of tracked("outer", log)) {
	for (const b of tracked("inner", log)) break;
	break;
}
console.log("nested loops:", log.join(","));
