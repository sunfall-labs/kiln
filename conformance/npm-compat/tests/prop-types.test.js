// kiln eval smoke test (authored for kiln, not from porffor).
const PropTypes = require('prop-types');
console.log(typeof PropTypes.string, typeof PropTypes.checkPropTypes);
PropTypes.checkPropTypes({name: PropTypes.string.isRequired}, {name: 'Ada'}, 'prop', 'UserCard');
console.log('valid-ok');
PropTypes.checkPropTypes({name: PropTypes.string.isRequired}, {age: 3}, 'prop', 'UserCard');
console.log('invalid-ok');
PropTypes.checkPropTypes(
  {tags: PropTypes.arrayOf(PropTypes.number)},
  {tags: [1, 2, 3]},
  'prop',
  'List',
);
console.log('array-ok');
