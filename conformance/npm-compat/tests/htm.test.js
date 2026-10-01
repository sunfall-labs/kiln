// kiln eval smoke test (authored for kiln, not from porffor).
const htm = require('htm');
const html = htm.bind((tag, props, ...children) => ({tag, props, children}));
console.log(JSON.stringify(html`<div id=${'a'}>hi</div>`));
console.log(html`<ul>${[1, 2].map(n => html`<li>${n}</li>`)}</ul>`.children.length);
console.log(JSON.stringify(html`<input disabled=${true} />`.props));
console.log(JSON.stringify(html`<p>${'a'} and ${'b'}</p>`.children));
