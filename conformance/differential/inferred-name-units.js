// Inferred names use JavaScript UTF-16 units. A lone surrogate and U+FFFD are distinct keys,
// and an astral pair remains two code units when it becomes Function.prototype.name.
function codes(value) {
	const result = [];
	for (let index = 0; index < value.length; index += 1) result.push(value.charCodeAt(index));
	return result.join(',');
}

const high = "\ud800";
const low = "\udc00";
const replacement = "\ufffd";
const astral = "\ud83d\ude00";

const object = {
	[high]() {},
	[low]() {},
	[replacement]() {},
	[astral]() {},
};
console.log(
	"object",
	codes(object[high].name),
	codes(object[low].name),
	codes(object[replacement].name),
	codes(object[astral].name),
);

const classValue = class {
	[high]() {}
	[low]() {}
	[replacement]() {}
	[astral]() {}
};
const instance = new classValue();
console.log(
	"class",
	codes(instance[high].name),
	codes(instance[low].name),
	codes(instance[replacement].name),
	codes(instance[astral].name),
);

// This class mirrors the two independent backing states of auto-accessors while remaining
// executable by Node's oracle: distinct private slots cannot be merged by lossy display text.
class DistinctState {
	#high = 11;
	#replacement = 22;
	get [high]() { return this.#high; }
	get [replacement]() { return this.#replacement; }
}
const state = new DistinctState();
console.log("state", state[high], state[replacement]);
