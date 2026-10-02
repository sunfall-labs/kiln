// kiln-native: profile=native-release
// A cached collection method is valid only while the receiver still resolves the canonical
// built-in member. Warm the same call sites on intrinsic collections before using subclasses,
// own shadows, modified prototypes, and weak collections.
function setAt(target, key, value) {
	return target.set(key, value);
}

function getAt(target, key) {
	return target.get(key);
}

function hasAt(target, key) {
	return target.has(key);
}

function sizeOf(target) {
	return target.size;
}

function clearAt(target) {
	return target.clear();
}

function addAt(target, value) {
	return target.add(value);
}

function memberAt(target, key, ...args) {
	return target[key](...args);
}

function throwsTypeError(callback) {
	try {
		callback();
	} catch (error) {
		return error instanceof TypeError;
	}

	return false;
}

const intrinsic = new Map();
setAt(intrinsic, 'x', 1);
if (getAt(intrinsic, 'x') !== 1 || !hasAt(intrinsic, 'x') || sizeOf(intrinsic) !== 1) {
	throw new Error('intrinsic Map method fast paths changed behavior');
}
const dynamicMap = new Map();
if (memberAt(dynamicMap, 'set', 'x', 9) !== dynamicMap ||
	memberAt(dynamicMap, 'get', 'x') !== 9 || memberAt(dynamicMap, 'has', 'missing') !== false) {
	throw new Error('computed collection call cache reused a different member');
}

class CustomMap extends Map {
	constructor() {
		super();
		this.calls = 0;
	}

	set(key, value) {
		this.calls++;
		this.customValue = value;
		return this;
	}

	get(key) {
		this.calls++;
		return `custom:${key}`;
	}

	has(key) {
		this.calls++;
		return true;
	}

	get size() {
		this.calls++;
		return 73;
	}
}

const derived = new CustomMap();
setAt(derived, 'x', 2);
if (getAt(derived, 'x') !== 'custom:x' || !hasAt(derived, 'x') || sizeOf(derived) !== 73 ||
	derived.calls !== 4 || Map.prototype.has.call(derived, 'x')) {
	throw new Error('Map subclass overrides were bypassed by collection fast paths');
}

const own = new Map();
own.set = function (key, value) {
	this.ownValue = value;
	return this;
};
if (setAt(own, 'x', 3) !== own || own.ownValue !== 3 || Map.prototype.has.call(own, 'x')) {
	throw new Error('own Map method shadow was bypassed');
}

const originalHas = Map.prototype.has;
Map.prototype.has = function () { return 'patched'; };
try {
	if (hasAt(intrinsic, 'absent') !== 'patched') {
		throw new Error('Map.prototype mutation was bypassed');
	}
} finally {
	Map.prototype.has = originalHas;
}

const set = new Set();
addAt(set, 'x');
if (!set.has('x')) throw new Error('intrinsic Set add fast path changed behavior');
if (!throwsTypeError(() => setAt(set, 'y', 4)) ||
	!throwsTypeError(() => memberAt(set, 'get', 'x'))) {
	throw new Error('collection method cache crossed Map/Set method surfaces');
}

function deleteAt(target, key) {
	return target.delete(key);
}

function clearSetAt(target) {
	return target.clear();
}

class CustomSet extends Set {
	constructor() {
		super();
		this.calls = 0;
	}

	add(value) {
		this.calls++;
		this.customValue = value;
		return this;
	}

	has(value) {
		this.calls++;
		return true;
	}

	delete(value) {
		this.calls++;
		return value === 'custom';
	}

	clear() {
		this.calls++;
		this.wasCleared = true;
	}

	get size() {
		this.calls++;
		return 41;
	}
}

const customSet = new CustomSet();
addAt(customSet, 'custom');
if (!hasAt(customSet, 'not-in-native-storage') || !deleteAt(customSet, 'custom')) {
	throw new Error('Set subclass methods were bypassed');
}
clearSetAt(customSet);
if (sizeOf(customSet) !== 41 || customSet.calls !== 5 ||
	customSet.customValue !== 'custom' || !customSet.wasCleared ||
	Set.prototype.has.call(customSet, 'custom')) {
	throw new Error('Set subclass methods touched the intrinsic backing store');
}

const ownSet = new Set();
ownSet.add = function (value) {
	this.ownValue = value;
	return this;
};
if (addAt(ownSet, 'own') !== ownSet || ownSet.ownValue !== 'own' ||
	Set.prototype.has.call(ownSet, 'own')) {
	throw new Error('own Set method shadow was bypassed');
}

const weakMap = new WeakMap();
if (!throwsTypeError(() => setAt(weakMap, 1, 2)) ||
	!throwsTypeError(() => clearAt(weakMap))) {
	throw new Error('WeakMap fast path skipped key or method validation');
}

const weakSet = new WeakSet();
if (!throwsTypeError(() => addAt(weakSet, 1))) {
	throw new Error('WeakSet fast path skipped weak-key validation');
}

console.log('collection method fast paths respect overrides and collection brands');
