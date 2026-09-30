// kiln eval smoke test (authored for kiln, not from porffor).
import yargs from 'yargs';

const argv = yargs(['serve', '--port', '8080', '--verbose'])
  .command('serve', 'start the server')
  .option('port', { type: 'number', default: 3000 })
  .option('verbose', { type: 'boolean' })
  .parse();
console.log(argv._[0], argv.port, argv.verbose);

const argv2 = yargs([])
  .option('name', { type: 'string', demandOption: false })
  .parse();
console.log(argv2.name === undefined);

const argv3 = yargs(['--tags', 'a', '--tags', 'b']).option('tags', { type: 'array' }).parse();
console.log(JSON.stringify(argv3.tags));
