// Own-frame member reads must retain actual extra arguments and fall back to the
// observable arguments object after mutation, capture, or an inherited index.
function noFormals() {
  return [arguments.length, arguments[0], arguments[1], arguments[2]];
}

function invoke(callback, values) {
  return Reflect.apply(callback, null, values);
}

function curried() {
  if (arguments.length === 1) return () => arguments[0];
  return [arguments[0], arguments[1]];
}

function mutateMaterialized() {
  const before = arguments[0];
  const alias = arguments;
  alias.length = 7;
  alias[0] = 'changed';
  Object.defineProperty(alias, '1', {
    configurable: true,
    get() { return 'accessor'; }
  });
  return [before, arguments.length, arguments[0], arguments[1]];
}

function deletedOwnIndex() {
  const alias = arguments;
  delete alias[0];
  return arguments[0];
}

let inheritedZeroCalls = 0;
let inheritedOneCalls = 0;
let inheritedReceiver = false;
function missingZero() { return arguments[0]; }
function missingOne() { return arguments[1]; }
let missing;
let deleted;
try {
  Object.defineProperty(Object.prototype, '0', {
    configurable: true,
    get() { inheritedZeroCalls++; return 'inherited-zero'; }
  });
  Object.defineProperty(Object.prototype, '1', {
    configurable: true,
    get() {
      inheritedOneCalls++;
      inheritedReceiver = this.length === 1 && this[0] === 'first';
      return 'inherited-one';
    }
  });
  const zero = missingZero();
  const one = missingOne('first');
  deleted = deletedOwnIndex('removed');
  missing = { zero, one };
} finally {
  delete Object.prototype[0];
  delete Object.prototype[1];
}

const marker = { value: 'rooted' };
const callback = () => 'callable';
const token = Symbol('argument');
function representations() {
  const allocated = { nested: [1, 2, 3] };
  return [arguments[0] === marker, arguments[1] === callback,
    arguments[2] === token, Object.is(arguments[3], -0),
    arguments[0].value, arguments[1](), allocated.nested.length];
}

const escaped = curried('captured');
function unmapped(first) {
  first = 'parameter-write';
  return [arguments[0], arguments.length, first];
}
console.log(JSON.stringify({
  direct: noFormals('a', 'b', 'c'),
  generic: invoke(noFormals, ['d', 'e', 'f']),
  curried: escaped('different', 'own', 'arguments'),
  twoActuals: curried('left', 'right'),
  mutation: mutateMaterialized('original', 'second'),
  missing, deleted, inheritedZeroCalls, inheritedOneCalls, inheritedReceiver,
  unmapped: unmapped('original', 'extra'),
  representations: representations(marker, callback, token, -0)
}));
