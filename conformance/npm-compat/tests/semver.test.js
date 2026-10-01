// kiln npm functional test: semver
const semver = require('semver');

// 1. Parsing and validation
console.log('valid:', semver.valid('1.2.3'), semver.valid('  v1.2.3  '), semver.valid('invalid'));
console.log('clean:', semver.clean('  =v1.2.3   '));
console.log('parts:', semver.major('1.2.3-alpha.1'), semver.minor('1.2.3-alpha.1'), semver.patch('1.2.3-alpha.1'));
console.log('prerelease:', JSON.stringify(semver.prerelease('1.2.3-alpha.1')));

// 2. Increments
console.log('inc-major:', semver.inc('1.2.3', 'major'));
console.log('inc-minor:', semver.inc('1.2.3', 'minor'));
console.log('inc-patch:', semver.inc('1.2.3', 'patch'));
console.log('inc-prerelease:', semver.inc('1.2.3-alpha.0', 'prerelease'));

// 3. Comparisons and sorting
console.log('cmp-gt:', semver.gt('2.0.0', '1.9.9'));
console.log('cmp-lt:', semver.lt('1.0.0-alpha', '1.0.0-beta'));
console.log('cmp-eq:', semver.eq('1.0.0', 'v1.0.0'));
const versions = ['1.2.3', '0.1.0', '2.0.0', '1.0.0', '1.2.0'];
console.log('sort:', JSON.stringify(semver.sort(versions.slice())));
console.log('rsort:', JSON.stringify(semver.rsort(versions.slice())));

// 4. Ranges and satisfaction
console.log('satisfies-caret:', semver.satisfies('1.2.3', '^1.0.0'));
console.log('satisfies-tilde:', semver.satisfies('1.2.3', '~1.2.0'));
console.log('satisfies-compound:', semver.satisfies('2.3.4', '>=1.0.0 <3.0.0'));
console.log('satisfies-or:', semver.satisfies('3.0.0', '1.x || >=2.5.0'));

// 5. Min/Max satisfying
console.log('max-satisfying:', semver.maxSatisfying(versions, '^1.0.0'));
console.log('min-satisfying:', semver.minSatisfying(versions, '^1.0.0'));

// 6. Range relations
console.log('intersects:', semver.intersects('^1.0.0', '~1.2.0'));
console.log('subset:', semver.subset('~1.2.0', '^1.0.0'));

// 7. Coerce
const coerced = semver.coerce('node v14.8.0-rc1');
console.log('coerce:', coerced ? coerced.version : null);
