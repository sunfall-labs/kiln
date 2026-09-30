// kiln eval smoke test (authored for kiln, not from porffor).
import cac from 'cac';

const cli = cac('kiln-eval');
cli
  .command('build <src>', 'build a project')
  .option('--out <dir>', 'output dir', { default: 'dist' })
  .option('--minify', 'minify output')
  .action((src, options) => {
    console.log('action src:', src);
    console.log('action options:', JSON.stringify(options));
  });
cli.option('--global-flag', 'a global flag');
cli.help();
cli.version('1.2.3');

const parsed = cli.parse(['node', 'cli', 'build', 'src/index.js', '--minify'], { run: false });
console.log('parsed args:', JSON.stringify(parsed.args));
console.log('parsed options:', JSON.stringify(parsed.options));

const cli2 = cac('other');
cli2.command('run', 'run it').action(() => console.log('ran'));
console.log('commands:', JSON.stringify(cli2.commands.map((c) => c.name)));
console.log('has runMatchedCommand:', typeof cli2.runMatchedCommand === 'function');
