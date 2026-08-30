// The engine is `regress`, chosen because it targets ECMAScript semantics: backreferences,
// lookaround and sticky matching are all ordinary JavaScript and none of them is expressible in a
// linear-time engine.
console.log(/(\w)\1/.test('hello'), /(\w)\1/.test('helo'));
console.log(/foo(?=bar)/.test('foobar'), /foo(?!bar)/.test('foobar'));
console.log('a@example'.match(/(?<=@)\w+/)[0]);
console.log(/(?<year>\d{4})-(?<month>\d{2})/.exec('2026-08')[1]);

// `exec` returns an array carrying `index` and `input`, which is why an array needs somewhere to
// put a property that is not an element.
const found = /(\d+)-(\d+)/.exec('x 12-34 y');
console.log(found.length, found[0], found[1], found[2], found.index, found.input);
console.log(Array.isArray(found), found.map((s) => s.length).join(','));
console.log(/nope/.exec('abc'));

// `lastIndex` advances only for `g` and `y`, and resets when a search fails.
const global = /\d/g;
console.log(global.lastIndex, global.test('a1'), global.lastIndex, global.test('a1'), global.lastIndex);
const plain = /\d/;
console.log(plain.test('a1'), plain.lastIndex);

// A sticky pattern must match *at* the cursor rather than anywhere after it.
const sticky = /\d/y;
sticky.lastIndex = 0;
console.log(sticky.test('a1'), /\d/y.test('1a'));

// Flags and source are readable, and `toString` gives the literal back.
const re = /ab+c/gi;
console.log(re.source, re.flags, re.global, re.ignoreCase, re.multiline, String(re));

// `match` without `g` is `exec`; with `g` it is every matched text.
console.log(JSON.stringify('a1b22'.match(/\d+/)), 'a1b22'.match(/\d+/g).join(','));
console.log('abc'.match(/z/));

// `matchAll` yields full results.
const all = [...'a1b22'.matchAll(/(\d+)/g)];
console.log(all.length, all[0][1], all[1][1], all[0].index, all[1].index);

// Replacement: a function, and the `$` forms.
console.log('a1b2'.replace(/\d/g, (d) => '<' + d + '>'));
console.log('2026-08-29'.replace(/(\d+)-(\d+)-(\d+)/, '$3/$2/$1'));
console.log('abc'.replace(/b/, '[$&][$`][$\'][$$]'));
console.log('aaa'.replace(/a/g, 'b'), 'aaa'.replace(/a/, 'b'));
console.log('a.b'.replaceAll('.', '-'), 'a.b'.replace(/\./g, '-'));

// Split, including a capturing separator, which contributes its groups.
console.log('a,b;c'.split(/[,;]/).join('|'), 'a1b2c'.split(/(\d)/).join('|'));
console.log('abc'.split(/x/).length, 'abc'.search(/b/), 'abc'.search(/z/));

// Case-insensitive and multiline.
console.log(/ABC/i.test('abc'), 'x\ny'.match(/^y$/m)[0]);

// Unicode: an index is in UTF-16 units, which is what the language counts in.
const astral = '😀ab';
console.log(astral.match(/ab/).index, astral.length);
