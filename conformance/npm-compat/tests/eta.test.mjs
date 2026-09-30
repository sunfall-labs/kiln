// kiln eval smoke test (authored for kiln, not from porffor).
import {Eta} from 'eta';
const eta = new Eta();
console.log(eta.renderString('Hi <%= it.name %>!', {name: 'Ada'}));
console.log(eta.renderString('<%= it.a + it.b %>', {a: 2, b: 3}));
console.log(eta.renderString('<% for (const i of it.items) { %><%= i %>;<% } %>', {items: [1, 2]}));
console.log(eta.renderString('<%= it.missing ?? "dflt" %>', {}));
