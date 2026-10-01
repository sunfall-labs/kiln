// kiln eval smoke test (authored for kiln, not from porffor).
const TurndownService = require('turndown');
const t = new TurndownService();
console.log(JSON.stringify(t.turndown('<h1>Hi</h1>')));
console.log(JSON.stringify(t.turndown('<p>para <strong>bold</strong> and <em>it</em></p>')));
console.log(JSON.stringify(t.turndown('<ul><li>a</li><li>b</li></ul>')));
console.log(JSON.stringify(t.turndown('<a href="https://x.com">link</a>')));
