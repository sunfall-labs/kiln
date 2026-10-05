const escapeHtml = require('escape-html');

console.log('--- HTML Escaping ---');
console.log(escapeHtml('foo & bar'));
console.log(escapeHtml('<script>alert("XSS")</script>'));
console.log(escapeHtml("it's 'great' & <awesome>"));
console.log(escapeHtml('"double" and \'single\' quotes'));
console.log(escapeHtml('&&&& <<< >>> """ \'\'\''));

console.log('--- Fast path (no special chars) ---');
console.log(escapeHtml('hello world 12345!'));
console.log(escapeHtml(''));

console.log('--- Type Coercion ---');
console.log(escapeHtml(123));
console.log(escapeHtml(true));
console.log(escapeHtml(false));
console.log(escapeHtml({ toString: () => '<b>from object</b>' }));
