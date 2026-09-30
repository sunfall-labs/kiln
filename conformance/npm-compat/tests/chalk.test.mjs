// kiln eval smoke test (authored for kiln, not from porffor).
import chalk, {Chalk} from 'chalk';
const c = new Chalk({level: 2});
console.log(JSON.stringify(c.red('red')));
console.log(JSON.stringify(c.bold('bold')));
console.log(JSON.stringify(c.rgb(255, 128, 0)('orange')));
console.log(JSON.stringify(chalk.green('plain')));
console.log('defaultLevel:' + chalk.level);
