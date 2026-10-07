// Every spelling of a statically named actor module reaches the target in both engines. Each
// spelling names its own module, so a native build that failed to discover one of them cannot be
// rescued by another; ADR 0072 says a native build links the actor modules named statically.
import { spawnActor as spawn } from 'kiln:actors';
import * as actors from 'kiln:actors';
import { fileURLToPath } from 'node:url';

const show = async (label, owner) => {
  console.log(label, await owner.ref.call('increment', [1]));
  await owner.stop();
};

// A renamed import: the call is found by its binding, not by the spelling `spawnActor`.
await show('renamed', await spawn({
  module: new URL('./support/actor-name-renamed.mjs', import.meta.url), export: 'Counter', args: [1],
}));
// A namespace import.
await show('namespace', await actors.spawnActor({
  module: new URL('./support/actor-name-namespace.mjs', import.meta.url), export: 'Counter', args: [2],
}));
// Options held in a const binding.
const options = {
  module: new URL('./support/actor-name-options.mjs', import.meta.url), export: 'Counter', args: [3],
};
await show('options', await spawn(options));
// Options spread from a const binding.
const base = { module: new URL('./support/actor-name-spread.mjs', import.meta.url) };
await show('spread', await spawn({ ...base, export: 'Counter', args: [4] }));
// A file URL string, a path, and a fileURLToPath of the URL.
await show('href', await spawn({
  module: new URL('./support/actor-name-href.mjs', import.meta.url).href, export: 'Counter', args: [5],
}));
await show('pathname', await spawn({
  module: new URL('./support/actor-name-pathname.mjs', import.meta.url).pathname, export: 'Counter', args: [6],
}));
const filepath = fileURLToPath(new URL('./support/actor-name-filepath.mjs', import.meta.url));
await show('filepath', await spawn({ module: filepath, export: 'Counter', args: [7] }));
// A name computed at run time reaches a module linked by another spelling.
const kind = ['opt', 'ions'].join('');
await show('computed', await spawn({
  module: new URL(`./support/actor-name-${kind}.mjs`, import.meta.url), export: 'Counter', args: [8],
}));
// A relative string is refused in both engines: the library cannot see its caller's URL.
try { await spawn({ module: './support/actor-name-options.mjs', export: 'Counter' }); }
catch (error) { console.log('relative', error.name); }
