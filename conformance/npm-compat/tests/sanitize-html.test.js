// kiln eval smoke test (authored for kiln, not from porffor).
const sanitizeHtml = require('sanitize-html');

console.log(sanitizeHtml('<p>Hello <script>alert(1)</script></p>'));
console.log(
  sanitizeHtml('<a href="javascript:evil()">x</a>', {
    allowedTags: ['a'],
    allowedAttributes: { a: ['href'] },
  })
);
console.log(sanitizeHtml('<b>bold</b> <i>ital</i>', { allowedTags: ['b'] }));
console.log(sanitizeHtml('<div class="x">text</div>', { allowedAttributes: false }));
console.log(sanitizeHtml.defaults.allowedTags.includes('iframe'));
