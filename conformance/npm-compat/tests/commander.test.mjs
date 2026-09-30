// kiln eval smoke test (authored for kiln, not from porffor).
import { Command } from 'commander';
const p = new Command();
p.option('-p, --port <n>', 'port', '3000').option('-v, --verbose', 'verbose');
p.argument('[name]', 'name');
p.parse(['node', 't', '--port', '8080', '-v', 'kiln']);
console.log(JSON.stringify(p.opts()));
console.log(JSON.stringify(p.args));
const q = new Command();
q.option('--count <n>', 'count', parseInt);
q.parse(['node', 't', '--count', '42']);
console.log(JSON.stringify(q.opts()));
