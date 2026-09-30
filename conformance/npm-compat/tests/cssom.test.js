// kiln eval smoke test (authored for kiln, not from porffor).
const cssom = require('cssom');
const sheet = cssom.parse('body { margin: 0; color: red; } .a { padding: 2px }');
console.log(sheet.cssRules.length);
console.log(sheet.cssRules[0].selectorText);
console.log(sheet.cssRules[0].style.getPropertyValue('color'));
console.log(sheet.cssRules[1].style.getPropertyValue('padding'));
console.log(cssom.parse('a { color: blue }').cssRules[0].style.cssText);
