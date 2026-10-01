// kiln eval smoke test (authored for kiln, not from porffor).
const sax = require('sax');

const events = [];
const parser = sax.parser(true);
parser.onopentag = (node) => events.push(['open', node.name, JSON.stringify(node.attributes)]);
parser.ontext = (text) => events.push(['text', text.trim()]);
parser.onclosetag = (name) => events.push(['close', name]);
parser.write('<root a="1"><child>x</child></root>');
parser.close();

for (const e of events) console.log(JSON.stringify(e));

const strict = sax.parser(true);
let errored = false;
strict.onerror = () => {
  errored = true;
  strict.resume();
};
strict.write('<a><b></a>').close();
console.log(errored);
