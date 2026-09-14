// Symbol descriptions and the global registry use the same UTF-16 units as ordinary strings.
function codes(value) {
	const result = [];
	for (let index = 0; index < value.length; index += 1) result.push(value.charCodeAt(index));
	return result.join(',');
}

const high = "\ud800";
const replacement = "\ufffd";
const highSymbol = Symbol.for(high);
const replacementSymbol = Symbol.for(replacement);
console.log(
	"keys",
	codes(Symbol.keyFor(highSymbol)),
	codes(Symbol.keyFor(replacementSymbol)),
	highSymbol === replacementSymbol,
);
console.log("missing", codes(Symbol.keyFor(Symbol.for())));
try {
	Symbol.keyFor();
} catch (error) {
	console.log("keyFor", error.name);
}

const object = {
	[highSymbol]() {},
	[replacementSymbol]() {},
};
console.log("methods", codes(object[highSymbol].name), codes(object[replacementSymbol].name));
