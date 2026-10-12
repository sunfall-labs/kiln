import { run } from 'kiln:process';

const result = await run('printf', ['Hello from kiln\n']);
console.log(result.exitCode);
console.log(new TextDecoder().decode(result.stdout).trim());
