// kiln eval smoke test (authored for kiln, not from porffor).
const escapeHtml = require('escape-html');
console.log(escapeHtml('<a href="x">y</a>'));
console.log(escapeHtml("it's & <b>bold</b>"));
console.log(escapeHtml('plain text'));
console.log(escapeHtml('<script>alert(1)</script>'));
