// A computed property after a spread shares one definition site across keys. The incoming object
// has the same shape every time, but the learned transition/slot is valid only for its original key.
function withKey(source, key, value) {
  return { ...source, [key]: value };
}

const source = { _: ['build', 'src/main.js'] };
const underscore = withKey(source, '_', ['again']);
const minify = withKey(source, 'minify', true);
const other = withKey(source, 'other', 7);
const minifyAgain = withKey(source, 'minify', false);
console.log(JSON.stringify(underscore));
console.log(JSON.stringify(minify));
console.log(JSON.stringify(other));
console.log(JSON.stringify(minifyAgain));
