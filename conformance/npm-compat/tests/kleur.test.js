// kiln eval smoke test (authored for kiln, not from porffor).
const kleur = require('kleur');
kleur.enabled = true;
console.log(JSON.stringify(kleur.red('red')));
console.log(JSON.stringify(kleur.bold().blue('bold blue')));
console.log(JSON.stringify(kleur.green().underline('x')));
console.log(kleur.enabled);
kleur.enabled = false;
console.log(JSON.stringify(kleur.red('plain')));
